#[macro_export]
macro_rules! dbg_file {
    ($($arg:tt)*) => {{
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/arc.log")
        {
            let _ = writeln!(f, $($arg)*);
        }
    }};
}
