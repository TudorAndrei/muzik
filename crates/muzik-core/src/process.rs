use std::ffi::OsStr;
use std::process::Command;

pub const BACKGROUND_NICENESS: u8 = 10;

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
}
