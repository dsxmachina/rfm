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
        if out.len() > len {
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

impl<T: std::fmt::Display> ExactWidth for T {}
