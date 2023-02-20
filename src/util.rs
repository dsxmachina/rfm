pub fn file_size_str(file_size: u64) -> String {
    match file_size {
        0..=1023 => format!("{} B", file_size),
        1024..=1048575 => format!("{:.1} K", (file_size as f64) / 1024.),
        1048576..=1073741823 => format!("{:.1} M", (file_size as f64) / 1048576.),
        1073741824..=1099511627775 => format!("{:.2} G", (file_size as f64) / 1073741824.),
        1099511627776..=1125899906842623 => {
            format!("{:.3} T", (file_size as f64) / 1099511627776.)
        }
        1125899906842624..=1152921504606846976 => {
            format!("{:4} P", (file_size as f64) / 1125899906842624.)
        }
        _ => format!("too big"),
    }
}
