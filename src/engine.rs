pub mod commands;
pub mod opener;
pub mod symbols;

pub use opener::OpenEngine;
pub use symbols::SymbolEngine;

// pub mod zoxide {}

pub mod shell {
    pub use std::process::{Command, Stdio};
    use std::{
        collections::VecDeque,
        io::{BufRead, BufReader},
        path::PathBuf,
        time::Duration,
    };

    use anyhow::{Context, Result};
    use log::{info, warn};
    use tokio::{
        sync::mpsc,
        task::{spawn_blocking, JoinHandle},
        time::{interval, MissedTickBehavior},
    };

    use super::commands::ShellCmd;

    pub struct Execute {
        shell_cmd: ShellCmd,
        items: Vec<PathBuf>,
    }

    impl Execute {
        pub fn new(cmd: String, args: String, multi: bool, items: Vec<PathBuf>) -> Self {
            Execute {
                shell_cmd: ShellCmd { cmd, args, multi },
                items,
            }
        }
    }

    impl ShellCmd {
        pub fn into_execute(self, items: Vec<PathBuf>) -> Execute {
            Execute {
                shell_cmd: self,
                items,
            }
        }
    }

    pub enum ExecMsg {
        /// The task is making some progress (used to visualize spinner)
        Progress,
        /// New task is queued (only happens if another task is still running)
        Queued,
        /// Task has finished
        Finished,
    }

    pub struct ShellExecutor {
        input_rx: mpsc::UnboundedReceiver<Execute>,
        result_tx: mpsc::Sender<ExecMsg>,
        queue: VecDeque<Execute>,
        task_handle: Option<JoinHandle<Result<()>>>,
    }

    fn execute_cmd(exec: Execute) -> Result<()> {
        let mut proc = Command::new(&exec.shell_cmd.cmd);
        proc.arg(exec.shell_cmd.args);
        proc.arg("--");
        for path in exec.items.iter().flat_map(|p| p.canonicalize()) {
            proc.arg(path);
        }
        let mut child = proc.spawn()?;
        let status = child.wait()?;
        if status.success() {
            info!("{} finished", exec.shell_cmd.cmd);
        } else {
            warn!("{} failed: {}", exec.shell_cmd.cmd, status);
        }
        Ok(())
    }

    impl ShellExecutor {
        pub fn new(
            input_rx: mpsc::UnboundedReceiver<Execute>,
            result_tx: mpsc::Sender<ExecMsg>,
        ) -> Self {
            ShellExecutor {
                input_rx,
                result_tx,
                queue: VecDeque::new(),
                task_handle: None,
            }
        }

        pub async fn run(mut self) -> Result<()> {
            let mut progress_timer = interval(Duration::from_millis(500));
            progress_timer.set_missed_tick_behavior(MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    biased;
                    _ = progress_timer.tick() => {
                        // Send progress message, in case there is a running task
                        if self.task_handle.is_some() {
                            self.result_tx.send(ExecMsg::Progress).await?;
                        }
                        info!("--- ping");
                    }
                    result = self.input_rx.recv() => {
                        let exec = result.context("channel closed")?;
                        if self.task_handle.is_some() {
                            self.queue.push_back(exec);
                            self.result_tx.send(ExecMsg::Queued).await?;
                        } else {
                            self.task_handle = Some(spawn_blocking(move || execute_cmd(exec)));
                            self.result_tx.send(ExecMsg::Progress).await?;
                        }
                    }
                    // Await the task_handle if it is Some
                    _ = async {
                        if let Some(handle) = self.task_handle.take() {
                            // TODO: Use SHUTDOWN_FLAG (somehow) to abort long running task
                            if let Err(err) = handle.await {
                                warn!("Task failed: {:?}", err);
                            }
                        }
                    }, if self.task_handle.is_some() => {
                        // At this point the task is done and can be reset
                        // if let Some()
                        info!("task finished");
                        self.result_tx.send(ExecMsg::Finished).await?;
                    }
                }
            }
        }
    }

    // pub fn exec_fzf() -> Result<()> {
    //     let mut handle = Command::new("fzf")
    //         .stdin(Stdio::piped())
    //         .stdout(Stdio::piped())
    //         .spawn()?;

    //     let stdin = handle
    //         .stdin
    //         .as_mut()
    //         .context("failed to connect to stdin")?;

    //     let stdout = handle
    //         .stdout
    //         .as_mut()
    //         .context("failed to connect to stdout")?;

    //     let reader = BufReader::new(stdout);
    //     for line in reader.lines() {
    //         println!("{}", line?);
    //     }

    //     handle.kill()?;

    //     Ok(())
    // }
}
