use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use color_eyre::eyre::{Context, Result};

use crate::config::DEFAULT_REQUEST_DELAY_MS;
use crate::github_debug;

const THROTTLE_FILE_NAME: &str = ".polyresearch-throttle";

static REQUEST_THROTTLE: OnceLock<RequestThrottle> = OnceLock::new();

#[derive(Debug, Clone)]
struct RequestThrottle {
    request_delay: Duration,
    state_path: PathBuf,
}

impl RequestThrottle {
    fn new(request_delay_ms: u64) -> Self {
        Self::with_path(request_delay_ms, default_state_path())
    }

    fn with_path(request_delay_ms: u64, state_path: PathBuf) -> Self {
        Self {
            request_delay: Duration::from_millis(request_delay_ms),
            state_path,
        }
    }
}

pub fn init(request_delay_ms: u64) {
    let _ = REQUEST_THROTTLE.get_or_init(|| RequestThrottle::new(request_delay_ms));
}

pub fn acquire_request_slot() -> Result<()> {
    let throttle = REQUEST_THROTTLE.get_or_init(|| RequestThrottle::new(DEFAULT_REQUEST_DELAY_MS));
    acquire_request_slot_with_config(throttle)
}

fn acquire_request_slot_with_config(throttle: &RequestThrottle) -> Result<()> {
    if throttle.request_delay.is_zero() {
        return Ok(());
    }

    if let Some(parent) = throttle.state_path.parent() {
        fs::create_dir_all(parent).wrap_err_with(|| {
            format!("failed to create throttle directory `{}`", parent.display())
        })?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&throttle.state_path)
        .wrap_err_with(|| {
            format!(
                "failed to open throttle state file `{}`",
                throttle.state_path.display()
            )
        })?;
    file.lock().wrap_err_with(|| {
        format!(
            "failed to lock throttle state file `{}`",
            throttle.state_path.display()
        )
    })?;

    let pace_result = pace_locked_file(&mut file, throttle.request_delay);
    let unlock_result = file.unlock().wrap_err_with(|| {
        format!(
            "failed to unlock throttle state file `{}`",
            throttle.state_path.display()
        )
    });

    pace_result?;
    unlock_result?;
    Ok(())
}

fn pace_locked_file(file: &mut File, request_delay: Duration) -> Result<()> {
    if let Some(last_request_at) = read_last_request(file)? {
        if let Some(wait) = remaining_delay(last_request_at, request_delay) {
            github_debug::log_throttle_wait(wait);
            thread::sleep(wait);
        }
    }

    write_last_request(file, SystemTime::now())
}

fn read_last_request(file: &mut File) -> Result<Option<SystemTime>> {
    file.seek(SeekFrom::Start(0))
        .wrap_err("failed to rewind throttle state file")?;

    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .wrap_err("failed to read throttle state file")?;

    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let millis = match trimmed.parse::<u64>() {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };

    Ok(UNIX_EPOCH.checked_add(Duration::from_millis(millis)))
}

fn remaining_delay(last_request_at: SystemTime, request_delay: Duration) -> Option<Duration> {
    let elapsed = SystemTime::now().duration_since(last_request_at).ok()?;
    if elapsed < request_delay {
        Some(request_delay - elapsed)
    } else {
        None
    }
}

fn write_last_request(file: &mut File, timestamp: SystemTime) -> Result<()> {
    let millis = timestamp
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    file.set_len(0)
        .wrap_err("failed to truncate throttle state file")?;
    file.seek(SeekFrom::Start(0))
        .wrap_err("failed to rewind throttle state file for writing")?;
    write!(file, "{millis}").wrap_err("failed to write throttle state file")?;
    file.flush()
        .wrap_err("failed to flush throttle state file")?;
    Ok(())
}

fn default_state_path() -> PathBuf {
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(THROTTLE_FILE_NAME))
        .unwrap_or_else(|| env::temp_dir().join(THROTTLE_FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process;
    use std::time::Instant;

    #[test]
    fn second_request_waits_for_the_configured_gap() {
        let path = unique_temp_path("delay");
        let throttle = RequestThrottle::with_path(40, path.clone());

        acquire_request_slot_with_config(&throttle).unwrap();
        let start = Instant::now();
        acquire_request_slot_with_config(&throttle).unwrap();

        assert!(start.elapsed() >= Duration::from_millis(25));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn invalid_state_file_is_treated_as_empty() {
        let path = unique_temp_path("invalid");
        fs::write(&path, "not-a-timestamp").unwrap();

        let throttle = RequestThrottle::with_path(5, path.clone());
        acquire_request_slot_with_config(&throttle).unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.trim().parse::<u128>().is_ok());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn zero_delay_skips_creating_the_state_file() {
        let path = unique_temp_path("zero");
        let throttle = RequestThrottle::with_path(0, path.clone());

        acquire_request_slot_with_config(&throttle).unwrap();

        assert!(!path.exists());
    }

    #[test]
    fn older_requests_do_not_overflow_or_wait() {
        let last_request_at = UNIX_EPOCH + Duration::from_secs(1);
        assert_eq!(
            remaining_delay(last_request_at, Duration::from_millis(5)),
            None
        );
    }

    fn unique_temp_path(label: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "polyresearch-throttle-{label}-{}-{}",
            process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
