use std::collections::VecDeque;
use std::process::Command;

use log::{debug, error, info, warn};
use tokio::sync::{mpsc, watch};

use super::types::{QueueStatus, QueuedCommand};

/// Executes queued commands in the background, one at a time.
pub struct CommandExecutor {
    /// Receiver for incoming commands
    rx: mpsc::UnboundedReceiver<QueuedCommand>,
    /// Queue of pending commands
    queue: VecDeque<QueuedCommand>,
    /// Sender for status updates (for UI widget)
    status_tx: watch::Sender<QueueStatus>,
}

impl CommandExecutor {
    pub fn new(
        rx: mpsc::UnboundedReceiver<QueuedCommand>,
        status_tx: watch::Sender<QueueStatus>,
    ) -> Self {
        Self {
            rx,
            queue: VecDeque::new(),
            status_tx,
        }
    }

    /// Update the status channel with current queue state
    fn update_status(&self, active: Option<&str>) {
        let status = QueueStatus {
            active: active.map(|s| s.to_string()),
            queued_count: self.queue.len(),
        };
        // Ignore send errors - receiver may have been dropped
        let _ = self.status_tx.send(status);
    }

    /// Execute a single command
    async fn execute_command(cmd: QueuedCommand) -> std::io::Result<std::process::Output> {
        let command = cmd.cmd.clone();
        let working_dir = cmd.working_dir.clone();

        tokio::task::spawn_blocking(move || {
            Command::new("sh")
                .arg("-c")
                .arg(&command)
                .current_dir(&working_dir)
                .output()
        })
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
    }

    /// Main execution loop
    pub async fn run(mut self) {
        info!("CommandExecutor started");

        loop {
            // If queue is empty, wait for new commands
            if self.queue.is_empty() {
                self.update_status(None);

                match self.rx.recv().await {
                    Some(cmd) => {
                        debug!("Received command: {}", cmd.name);
                        self.queue.push_back(cmd);
                    }
                    None => {
                        // Channel closed, shut down
                        info!("CommandExecutor shutting down");
                        return;
                    }
                }
            }

            // Process the next command in the queue
            if let Some(cmd) = self.queue.pop_front() {
                let name = cmd.name.clone();
                info!("Executing command '{}': {}", name, cmd.cmd);
                self.update_status(Some(&name));

                match Self::execute_command(cmd).await {
                    Ok(output) => {
                        // Log stdout if non-empty
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        if !stdout.trim().is_empty() {
                            for line in stdout.lines().take(10) {
                                debug!("[{}] {}", name, line);
                            }
                            if stdout.lines().count() > 10 {
                                debug!("[{}] ... ({} more lines)", name, stdout.lines().count() - 10);
                            }
                        }

                        // Log stderr if non-empty
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        if !stderr.trim().is_empty() {
                            for line in stderr.lines().take(10) {
                                warn!("[{}] {}", name, line);
                            }
                        }

                        // Log exit status
                        if output.status.success() {
                            info!("Command '{}' completed successfully", name);
                        } else {
                            let code = output.status.code().unwrap_or(-1);
                            error!("Command '{}' failed with exit code {}", name, code);
                        }
                    }
                    Err(e) => {
                        error!("Command '{}' failed to execute: {}", name, e);
                    }
                }
            }

            // Check for new commands without blocking
            while let Ok(cmd) = self.rx.try_recv() {
                debug!("Queued command: {}", cmd.name);
                self.queue.push_back(cmd);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_execute_simple_command() {
        let cmd = QueuedCommand {
            name: "test".to_string(),
            cmd: "echo hello".to_string(),
            working_dir: PathBuf::from("/tmp"),
        };

        let result = CommandExecutor::execute_command(cmd).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hello");
    }

    #[tokio::test]
    async fn test_execute_failing_command() {
        let cmd = QueuedCommand {
            name: "test".to_string(),
            cmd: "exit 1".to_string(),
            working_dir: PathBuf::from("/tmp"),
        };

        let result = CommandExecutor::execute_command(cmd).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        assert!(!output.status.success());
    }
}
