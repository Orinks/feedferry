use std::process::Command;

use anyhow::{Context, Result};

#[derive(Debug, Clone, Default)]
pub struct Accessibility {
    pub screen_reader: bool,
    pub speech_command: Option<String>,
}

impl Accessibility {
    pub fn new(screen_reader: bool, speech_command: Option<String>) -> Self {
        Self {
            screen_reader,
            speech_command,
        }
    }

    pub fn status(&self, message: impl AsRef<str>) -> Result<()> {
        let message = message.as_ref();
        if self.screen_reader {
            eprintln!("Status: {message}");
        }

        if let Some(command) = &self.speech_command {
            run_speech_command(command, message)
                .with_context(|| format!("failed to run speech command `{command}`"))?;
        }

        Ok(())
    }

    pub fn should_use_linear_output(&self) -> bool {
        self.screen_reader
    }
}

fn run_speech_command(command_line: &str, message: &str) -> Result<()> {
    let mut parts = command_line.split_whitespace();
    let Some(program) = parts.next() else {
        return Ok(());
    };

    let mut command = Command::new(program);
    for arg in parts {
        command.arg(arg);
    }
    command.arg(message);
    command.spawn()?.wait()?;
    Ok(())
}

pub fn accessibility_help() -> &'static str {
    "FeedFerry accessibility guide\n\n\
The desktop app uses labeled GUI controls, a selectable plain-text reader pane, keyboard shortcuts, and optional status announcements.\n\
The feedferry-cli companion remains command-based for automation and linear screen-reader output.\n\
Use --screen-reader in the CLI to request linear item blocks and explicit Status lines.\n\
Use --speech-command to send status messages to a command such as spd-say, say, or a custom script.\n\
Examples:\n\
  feedferry-cli --screen-reader items --unread\n\
  feedferry-cli --screen-reader --speech-command spd-say update\n\
  feedferry-cli --screen-reader read 42\n\n\
All CLI commands can be driven from the shell, shell history, aliases, and scripts."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_accessibility_is_not_linear() {
        let ax = Accessibility::default();
        assert!(!ax.should_use_linear_output());
    }

    #[test]
    fn screen_reader_flag_enables_linear_output() {
        let ax = Accessibility::new(true, None);
        assert!(ax.should_use_linear_output());
    }
}
