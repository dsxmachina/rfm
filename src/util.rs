use std::{
    error::Error,
    path::{Path, PathBuf},
};

use fs_extra::dir::CopyOptions;
use notify_rust::Notification;

pub fn file_size_str(file_size: u64) -> String {
    match file_size {
        0..=1023 => format!("{file_size} B"),
        1024..=1048575 => format!("{:.1} K", (file_size as f64) / 1024.),
        1048576..=1073741823 => format!("{:.1} M", (file_size as f64) / 1048576.),
        1073741824..=1099511627775 => format!("{:.2} G", (file_size as f64) / 1073741824.),
        1099511627776..=1125899906842623 => {
            format!("{:.3} T", (file_size as f64) / 1099511627776.)
        }
        1125899906842624..=1152921504606846976 => {
            format!("{:4} P", (file_size as f64) / 1125899906842624.)
        }
        _ => "too big".to_string(),
    }
}

pub trait ExactWidth: std::fmt::Display {
    fn exact_width(&self, len: usize) -> String {
        let mut out = format!("{:len$}", self);
        // We have to truncate the name
        if out.chars().count() > len {
            // FIX: If name_len does not lie on a char boundary,
            // the truncate function will panic
            if out.is_char_boundary(len) {
                out.truncate(len);
            } else {
                // This is stupidly inefficient, but cannot panic.
                while out.len() > len {
                    out.pop();
                }
            }
            out.pop();
            out.push('~');
        }
        out
    }
}

// lazy_static! {
//     static ref RE: Regex = Regex::new("\x1B\\[[0-9;]*m").expect("Failed to compile regex");
// }

// /// Counts the actual characters in a string with ansi escape codes
// pub fn count_actual_chars(input: &str) -> usize {
//     let stripped_string = RE.replace_all(input, "");
//     stripped_string.chars().count()
// }
pub fn truncate_with_color_codes(input: &str, limit: usize) -> String {
    let mut result = String::new();
    let mut char_count = 0;
    let mut escape = false;
    let mut codes = Vec::new();

    for c in input.chars() {
        if c == '\x1B' {
            escape = true;
        }

        if escape {
            if c == 'm' {
                escape = false;
                let code = &input[result.len()..result.len() + c.len_utf8()];
                if code != "\x1B[0m" {
                    // Not a reset code
                    codes.push(code);
                } else {
                    codes.clear(); // Reset code clears the stack
                }
            }
            result.push(c);
        } else if char_count < limit {
            result.push(c);
            char_count += 1;
        } else {
            break; // Reached the limit
        }
    }

    if char_count >= limit {
        // Append a reset code if we have any codes in the stack to close them
        if !codes.is_empty() {
            result.push_str("\x1B[0m");
        }
    }
    result
}

impl<T: std::fmt::Display> ExactWidth for T {}

/// Calculates the destination path when we want to copy or move items from 'source' to 'destination'.
///
/// Note: Destination must be a directory, otherwise this function will fail.
pub fn get_destination<P, Q>(source: P, destination: Q) -> Result<PathBuf, std::io::Error>
where
    P: AsRef<Path>,
    Q: AsRef<Path>,
{
    let from = source.as_ref();
    let to = destination.as_ref();
    if !to.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("{} is not a directory", to.display()),
        ));
    }
    let mut dest_name = from
        .file_name()
        .and_then(|p| p.to_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let mut result = to.join(&dest_name);
    // Append underscores until the name exists
    while result.exists() {
        dest_name.push('_');
        result = to.join(&dest_name);
    }
    Ok(result)
}

pub fn check_filename<P, Q, S>(
    source: P,
    destination: Q,
    extension: S,
) -> Result<PathBuf, std::io::Error>
where
    P: AsRef<Path>,
    Q: AsRef<Path>,
    S: AsRef<str>,
{
    let from = source.as_ref();
    let to = destination.as_ref();
    let extension = extension.as_ref();
    if !to.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("{} is not a directory", to.display()),
        ));
    }
    let mut dest_base = from
        .file_stem()
        .and_then(|p| p.to_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let dest_name = format!("{dest_base}.{extension}");
    let mut result = to.join(&dest_name);

    // Append underscores until the name exists
    while result.exists() {
        dest_base.push('_');
        let dest_name = format!("{dest_base}.{extension}");
        result = to.join(&dest_name);
    }
    Ok(result)
}

pub fn move_item<P, Q>(source: P, destination: Q) -> Result<(), Box<dyn Error>>
where
    P: AsRef<Path>,
    Q: AsRef<Path>,
{
    let from = source.as_ref();
    let dest_name = from
        .file_name()
        .and_then(|p| p.to_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    // If destination is the directory of from, don't do anything
    if from == destination.as_ref().join(dest_name) {
        Notification::new()
            .summary("from and to are identical")
            .show()
            .unwrap();
        return Ok(());
    }
    let to = get_destination(&source, destination)?;
    std::fs::rename(from, to)?;
    Ok(())
}

pub fn copy_item<P, Q>(source: P, destination: Q) -> Result<(), Box<dyn Error>>
where
    P: AsRef<Path>,
    Q: AsRef<Path>,
{
    let from = source.as_ref();
    let to = get_destination(&source, destination)?;
    if from.is_dir() {
        fs_extra::dir::copy(from, to, &CopyOptions::default().copy_inside(true))?;
    } else {
        std::fs::copy(from, to)?;
    }
    Ok(())
}

/// Query the XDG Config Home (usually ~/.config) according to
/// https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html
pub fn xdg_config_home() -> Result<PathBuf, Box<dyn Error>> {
    match std::env::var("XDG_CONFIG_HOME") {
        Ok(xdg_config) => Ok(PathBuf::from(xdg_config)),
        Err(_) => match std::env::var("HOME") {
            Ok(home) => Ok(PathBuf::from(home).join(".config")),
            Err(_) => Err(
                "Neither the XDG_CONFIG_HOME nor the HOME environment variable was set."
                    .to_string(),
            )?,
        },
    }
}
