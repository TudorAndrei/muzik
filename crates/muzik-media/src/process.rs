use process_wrap::std::{ChildWrapper, CommandWrap};
use std::ffi::OsStr;
use std::io;
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const BACKGROUND_NICENESS: u8 = 10;

#[derive(Debug, thiserror::Error)]
pub enum Stopped {
    #[error("cancelled")]
    Cancelled,
    #[error("timed out")]
    TimedOut,
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[cfg(unix)]
pub fn background_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new("nice");
    command
        .arg("-n")
        .arg(BACKGROUND_NICENESS.to_string())
        .arg(program);
    command
}

#[cfg(not(unix))]
pub fn background_command(program: impl AsRef<OsStr>) -> Command {
    Command::new(program)
}

pub fn spawn(command: Command) -> io::Result<Box<dyn ChildWrapper>> {
    let mut command = CommandWrap::from(command);
    #[cfg(unix)]
    command.wrap(process_wrap::std::ProcessGroup::leader());
    #[cfg(windows)]
    command.wrap(process_wrap::std::JobObject);
    command.spawn()
}

pub fn wait(
    child: &mut Box<dyn ChildWrapper>,
    timeout: Option<Duration>,
    cancelled: &AtomicBool,
) -> Result<ExitStatus, Stopped> {
    let started = Instant::now();
    loop {
        let stop = if cancelled.load(Ordering::SeqCst) {
            Some(Stopped::Cancelled)
        } else if timeout.is_some_and(|timeout| started.elapsed() >= timeout) {
            Some(Stopped::TimedOut)
        } else {
            None
        };
        if let Some(stop) = stop {
            let _ = child.kill();
            let _ = child.wait();
            return Err(stop);
        }
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn background_command_runs_the_program_at_lower_priority(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let output = background_command("sh")
            .arg("-c")
            .arg("ps -o nice= -p $$")
            .output()?;
        assert!(output.status.success());
        let niceness: i32 = String::from_utf8(output.stdout)?.trim().parse()?;
        assert!(niceness >= i32::from(BACKGROUND_NICENESS));
        Ok(())
    }

    #[test]
    fn a_stop_also_ends_the_programs_the_child_started() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let marker = directory.path().join("grandchild.pid");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!("sleep 30 & echo $! > '{}'; wait", marker.display()));
        let mut child = spawn(command)?;
        while !marker.exists() {
            std::thread::sleep(Duration::from_millis(10));
        }
        let stopped = wait(
            &mut child,
            Some(Duration::from_millis(100)),
            &AtomicBool::new(false),
        );
        assert!(matches!(stopped, Err(Stopped::TimedOut)));
        let pid = std::fs::read_to_string(&marker)?;
        std::thread::sleep(Duration::from_millis(100));
        let alive = Command::new("kill").arg("-0").arg(pid.trim()).output()?;
        assert!(!alive.status.success());

        let mut child = spawn(Command::new("true"))?;
        assert!(wait(&mut child, None, &AtomicBool::new(false))?.success());
        let mut sleep = Command::new("sleep");
        sleep.arg("30");
        let mut child = spawn(sleep)?;
        assert!(matches!(
            wait(&mut child, None, &AtomicBool::new(true)),
            Err(Stopped::Cancelled)
        ));
        Ok(())
    }
}
