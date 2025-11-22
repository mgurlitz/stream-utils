use std::path::PathBuf;
use std::process::Command;

pub fn run_segment_command(cmd_template: &str, filepath: &PathBuf, verbose: bool) {
    let filename = filepath.to_string_lossy();
    let cmd = cmd_template.replace("{}", &filename);

    if verbose {
        eprintln!("Running: {cmd}");
    }
    match Command::new("sh").arg("-c").arg(&cmd).status() {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!("Command exited with: {status}"),
        Err(e) => eprintln!("Failed to run command: {e}"),
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

pub fn run_exit_command(
    cmd_template: &str,
    duration_secs: u64,
    total_bytes: u64,
    output_dir: &PathBuf,
    verbose: bool,
) {
    // Get last two path components for %d placeholder
    let dir_str = {
        let components: Vec<_> = output_dir.components().collect();
        let len = components.len();
        if len >= 2 {
            format!(
                "{}/{}",
                components[len - 2].as_os_str().to_string_lossy(),
                components[len - 1].as_os_str().to_string_lossy()
            )
        } else if len == 1 {
            components[0].as_os_str().to_string_lossy().to_string()
        } else {
            ".".to_string()
        }
    };

    // Format duration as H:M:S (or M:S if < 60 minutes)
    let duration_str = {
        let hours = duration_secs / 3600;
        let minutes = (duration_secs % 3600) / 60;
        let seconds = duration_secs % 60;

        if hours > 0 {
            format!("{}:{:02}:{:02}", hours, minutes, seconds)
        } else {
            format!("{}:{:02}", minutes, seconds)
        }
    };

    let size_str = format_bytes(total_bytes);

    let cmd = cmd_template
        .replace("%d", &dir_str)
        .replace("%t", &duration_str)
        .replace("%s", &size_str)
        .replace("%b", &total_bytes.to_string())
        .replace("%m", &(total_bytes / 1024 / 1024).to_string());

    if verbose {
        eprintln!("Running exit command: {cmd}");
    }
    match Command::new("sh").arg("-c").arg(&cmd).status() {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!("Exit command exited with: {status}"),
        Err(e) => eprintln!("Failed to run exit command: {e}"),
    }
}

/// Async version that spawns the command without blocking
pub fn run_segment_command_async(
    cmd_template: String,
    filepath: PathBuf,
    verbose: bool,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        run_segment_command(&cmd_template, &filepath, verbose);
    })
}
