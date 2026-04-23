use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use color_eyre::eyre::{Context, Result, eyre};

pub const GUARD_ENV_VAR: &str = "POLYRESEARCH_ONCE_GUARD";

const ACTIVE_STATE: &str = "active";
const DONE_STATE: &str = "done";

fn guard_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".polyresearch").join(".once-guard")
}

pub fn create(repo_root: &Path) -> Result<PathBuf> {
    let path = guard_path(repo_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .wrap_err_with(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&path, ACTIVE_STATE)
        .wrap_err_with(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub fn mark_done() -> Result<()> {
    let Some(path) = env::var_os(GUARD_ENV_VAR) else {
        return Ok(());
    };

    match fs::write(&path, DONE_STATE) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).wrap_err_with(|| {
            format!(
                "failed to mark once guard done at {}",
                PathBuf::from(path).display()
            )
        }),
    }
}

pub fn check_cycle_limit() -> Result<()> {
    let Some(path) = env::var_os(GUARD_ENV_VAR) else {
        return Ok(());
    };
    let path = PathBuf::from(path);

    let state = match fs::read_to_string(&path) {
        Ok(state) => state,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .wrap_err_with(|| format!("failed to read once guard at {}", path.display()));
        }
    };

    if state.trim() == DONE_STATE {
        return Err(eyre!(
            "--once cycle limit reached: one thesis cycle is already complete"
        ));
    }

    Ok(())
}

pub fn remove(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn env_lock() -> &'static Mutex<()> {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    struct GuardEnvLock {
        _guard: MutexGuard<'static, ()>,
    }

    impl GuardEnvLock {
        fn lock_clean() -> Self {
            let guard = env_lock().lock().unwrap_or_else(|error| error.into_inner());
            clear_guard_env();
            Self { _guard: guard }
        }
    }

    impl Drop for GuardEnvLock {
        fn drop(&mut self) {
            clear_guard_env();
        }
    }

    fn clear_guard_env() {
        unsafe {
            env::remove_var(GUARD_ENV_VAR);
        }
    }

    fn set_guard_env(path: &Path) {
        unsafe {
            env::set_var(GUARD_ENV_VAR, path);
        }
    }

    fn temp_repo_root(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("polyresearch-cycle-guard-{name}-{unique}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn create_writes_active_guard() {
        let _env = GuardEnvLock::lock_clean();
        let repo_root = temp_repo_root("create-active");

        let path = create(&repo_root).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), ACTIVE_STATE);
        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn check_returns_ok_when_active() {
        let _env = GuardEnvLock::lock_clean();
        let repo_root = temp_repo_root("check-active");

        let path = create(&repo_root).unwrap();
        set_guard_env(&path);

        check_cycle_limit().unwrap();
        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn mark_done_transitions_guard() {
        let _env = GuardEnvLock::lock_clean();
        let repo_root = temp_repo_root("mark-done");

        let path = create(&repo_root).unwrap();
        set_guard_env(&path);
        mark_done().unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), DONE_STATE);
        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn check_returns_err_when_done() {
        let _env = GuardEnvLock::lock_clean();
        let repo_root = temp_repo_root("check-done");

        let path = create(&repo_root).unwrap();
        set_guard_env(&path);
        mark_done().unwrap();

        let error = check_cycle_limit().unwrap_err();
        assert!(error.to_string().contains("cycle limit"));
        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn check_returns_ok_when_env_unset() {
        let _env = GuardEnvLock::lock_clean();
        check_cycle_limit().unwrap();
    }

    #[test]
    fn remove_deletes_guard_file() {
        let _env = GuardEnvLock::lock_clean();
        let repo_root = temp_repo_root("remove");

        let path = create(&repo_root).unwrap();
        remove(&path);

        assert!(!path.exists(), "guard file should be removed");
        fs::remove_dir_all(repo_root).unwrap();
    }

    #[test]
    fn remove_noop_when_file_missing() {
        let _env = GuardEnvLock::lock_clean();
        let repo_root = temp_repo_root("remove-missing");
        let path = guard_path(&repo_root);

        remove(&path);

        assert!(!path.exists(), "missing guard file should stay absent");
        fs::remove_dir_all(repo_root).unwrap();
    }
}
