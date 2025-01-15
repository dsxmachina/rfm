use anyhow::anyhow;
use fs_extra::dir::CopyOptions;
use log::warn;
use std::{
    cmp::Ordering,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};
use time::OffsetDateTime;
use unicode_display_width::width as unicode_width;
use users::{get_group_by_gid, get_user_by_uid};

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

#[test]
fn exact_width_unicode() {
    let test_str = "Ｈｅｌｌｏ, ｗｏｒｌｄ!";
    println!("test-str={test_str}, width={}", unicode_width(&test_str));
    assert!(unicode_width("asdf") == 4);
    assert_eq!(unicode_width(&test_str.exact_width(4)), 4);
    let longer = test_str.exact_width(9);
    assert_eq!(unicode_width(&longer), 9, "{longer}");
    assert_eq!(unicode_width(&test_str.exact_width(2)), 2);
}

#[test]
fn exact_width_zero() {
    let test_str = "something";
    let empty = test_str.exact_width(0);
    assert!(empty.is_empty());
}

pub trait ExactWidth: std::fmt::Display {
    fn exact_width(&self, len: usize) -> String {
        // Edge-Case: len == 0
        if len == 0 {
            return String::new();
        }
        // Prepare output
        let mut out = format!("{}", self);
        let mut truncated = false;
        loop {
            let current_width = unicode_width(&out);
            match current_width.cmp(&(len as u64)) {
                Ordering::Less => {
                    // Pad with " " and return
                    let diff = (len as u64) - current_width;
                    if !truncated {
                        for _ in 0..diff {
                            out.push(' ');
                        }
                    } else {
                        for _ in 0..diff.saturating_sub(1) {
                            out.push(' ');
                        }
                        out.push('~');
                    }
                    break;
                }
                // NOTE: len == 0 is forbidden, otherwise we would .pop() forever
                Ordering::Equal => {
                    if !truncated {
                        break;
                    } else {
                        out.pop();
                    }
                }
                Ordering::Greater => {
                    // remove character and check again
                    out.pop();
                    truncated = true;
                }
            }
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
    let mut result = to.join(dest_name);

    // Append underscores until the name exists
    while result.exists() {
        dest_base.push('_');
        let dest_name = format!("{dest_base}.{extension}");
        result = to.join(&dest_name);
    }
    Ok(result)
}

pub fn move_item<P, Q>(source: P, destination: Q) -> anyhow::Result<()>
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
        warn!("from and to are identical");
        return Ok(());
    }
    let to = get_destination(&source, destination)?;
    std::fs::rename(from, to)?;
    Ok(())
}

pub fn copy_item<P, Q>(source: P, destination: Q) -> anyhow::Result<()>
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
pub fn xdg_config_home() -> anyhow::Result<PathBuf> {
    match std::env::var("XDG_CONFIG_HOME") {
        Ok(xdg_config) => Ok(PathBuf::from(xdg_config)),
        Err(_) => match std::env::var("HOME") {
            Ok(home) => Ok(PathBuf::from(home).join(".config")),
            Err(_) => Err(anyhow!(
                "Neither the XDG_CONFIG_HOME nor the HOME environment variable was set."
            ))?,
        },
    }
}

/// Returns the permissions and metadata for some selected path, if any.
///
/// The output is ready to be printed in the footer of the filemanager.
pub fn print_metadata(selected_path: Option<&Path>) -> (String, String) {
    if let Some(path) = selected_path {
        // TODO: Maybe we can put all of this into the DirElem and be done with it.
        if let Ok(metadata) = path.metadata() {
            let permissions = unix_mode::to_string(metadata.permissions().mode());
            let modified = metadata
                .modified()
                .map(OffsetDateTime::from)
                .map(|t| {
                    format!(
                        "{}-{:02}-{:02} {:02}:{:02}:{:02}",
                        t.year(),
                        u8::from(t.month()),
                        t.day(),
                        t.hour(),
                        t.minute(),
                        t.second()
                    )
                })
                .unwrap_or_else(|_| String::from("cannot read timestamp"));
            let user = get_user_by_uid(metadata.uid())
                .and_then(|u| u.name().to_str().map(String::from))
                .unwrap_or_default();
            let group = get_group_by_gid(metadata.gid())
                .and_then(|g| g.name().to_str().map(String::from))
                .unwrap_or_default();
            let size_str = file_size_str(metadata.size());
            let mime_type = mime_guess::from_path(path).first_raw().unwrap_or_default();
            let other = format!("{user} {group} {size_str} {modified} {mime_type}");
            (permissions, other)
        } else {
            ("------------".to_string(), "".to_string())
        }
    } else {
        ("------------".to_string(), "".to_string())
    }
}

// TODO: Use the device-id to check, if deletion actually just moves the file on the same disk.
// If not, the operation would be quite expensive, and we should then find another strategy.
//
// Trait to extract device ID in a cross-platform way
// pub trait CheckDeviceId {
//     fn device_id(&self) -> u64;
// }

// #[cfg(unix)]
// impl CheckDeviceId for std::fs::Metadata {
//     fn device_id(&self) -> u64 {
//         self.dev()
//     }
// }

// #[cfg(windows)]
// impl CheckDeviceId for std::fs::Metadata {
//     fn device_id(&self) -> u64 {
//         use std::os::windows::fs::MetadataExt;
//         self.volume_serial_number().unwrap_or(0)
//     }
// }
