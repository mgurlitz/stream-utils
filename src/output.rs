use chrono::Local;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct OutputFile {
    file: std::fs::File,
    file_extension: String,
    start_time: chrono::DateTime<Local>,
    segment_index: u32,
    segment_start: Instant,
    segment_duration: Duration,
    output_dir: PathBuf,
    total_bytes_written: u64,
}

impl OutputFile {
    pub fn new(
        file_extension: String,
        output_dir: PathBuf,
        segment_duration: Duration,
        verbose: bool,
    ) -> std::io::Result<Self> {
        let start_time = Local::now();
        // Find first available segment index (don't overwrite existing files)
        let mut segment_index = 0;
        loop {
            let filename = Self::format_filename(&start_time, segment_index, &file_extension);
            let path = output_dir.join(&filename);
            if !path.exists() {
                break;
            }
            segment_index += 1;
        }
        let filename = Self::format_filename(&start_time, segment_index, &file_extension);
        let path = output_dir.join(&filename);
        if verbose {
            eprintln!("Writing to: {}", path.display());
        }
        let file = std::fs::File::create(&path)?;

        Ok(Self {
            file,
            file_extension,
            start_time,
            segment_index,
            segment_start: Instant::now(),
            segment_duration,
            output_dir,
            total_bytes_written: 0,
        })
    }

    fn format_filename(
        start: &chrono::DateTime<Local>,
        index: u32,
        file_extension: &str,
    ) -> String {
        format!(
            "{}_{}.{}",
            start.format("%Y_%m_%d-%H_%M"),
            index,
            file_extension
        )
    }

    fn current_path(&self) -> PathBuf {
        self.output_dir.join(Self::format_filename(
            &self.start_time,
            self.segment_index,
            &self.file_extension,
        ))
    }

    pub fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.file.write_all(data)?;
        self.total_bytes_written += data.len() as u64;
        Ok(())
    }

    /// Check if rotation is needed. Returns the completed file path if rotated.
    pub fn maybe_rotate(&mut self, verbose: bool) -> std::io::Result<Option<PathBuf>> {
        if self.segment_start.elapsed() >= self.segment_duration {
            self.file.flush()?;
            let completed_path = self.current_path();

            self.segment_index += 1;
            let filename =
                Self::format_filename(&self.start_time, self.segment_index, &self.file_extension);
            let path = self.output_dir.join(&filename);
            if verbose {
                eprintln!("\nRotating to: {}", path.display());
            }
            self.file = std::fs::File::create(&path)?;
            self.segment_start = Instant::now();

            return Ok(Some(completed_path));
        }
        Ok(None)
    }

    /// Finalize the current segment (flush and return path)
    pub fn finalize(&mut self) -> std::io::Result<PathBuf> {
        self.file.flush()?;
        Ok(self.current_path())
    }

    /// Get total bytes written across all segments
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes_written
    }
}
