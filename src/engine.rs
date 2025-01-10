pub mod commands;
pub mod opener;
pub mod symbols;

pub use opener::OpenEngine;
pub use symbols::SymbolEngine;

// pub mod zoxide {}

// pub mod shell {
//     use std::io::{BufRead, BufReader};
//     pub use std::process::{Command, Stdio};

//     use anyhow::{Context, Result};

//     pub fn exec_fzf() -> Result<()> {
//         let mut handle = Command::new("fzf")
//             .stdin(Stdio::piped())
//             .stdout(Stdio::piped())
//             .spawn()?;

//         let stdin = handle
//             .stdin
//             .as_mut()
//             .context("failed to connect to stdin")?;

//         let stdout = handle
//             .stdout
//             .as_mut()
//             .context("failed to connect to stdout")?;

//         let reader = BufReader::new(stdout);
//         for line in reader.lines() {
//             println!("{}", line?);
//         }

//         handle.kill()?;

//         Ok(())
//     }
// }
