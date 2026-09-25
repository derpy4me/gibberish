//! Persistent Monotonic Nonce Manager with Write-Ahead Block Allocation (R6, R7, KTD3).

use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub const NONCE_BLOCK_SIZE: u64 = 1000;

#[derive(Debug, Serialize, Deserialize)]
struct NonceDiskState {
    last_persisted_epoch: u32,
    next_allocated_block: u64,
}

pub struct NonceManager {
    state_file_path: PathBuf,
    current_epoch: u32,
    current_counter: u64,
    block_limit: u64,
}

impl NonceManager {
    /// Initialize NonceManager using the default or specified file path.
    /// Default location: ~/.gibberish/nonce_state.json
    pub fn new(state_path: Option<PathBuf>) -> io::Result<Self> {
        let path = state_path.unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".gibberish").join("nonce_state.json")
        });

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as u32;

        let disk_state = if path.exists() {
            let content = fs::read_to_string(&path)?;
            serde_json::from_str::<NonceDiskState>(&content).unwrap_or(NonceDiskState {
                last_persisted_epoch: 0,
                next_allocated_block: 0,
            })
        } else {
            NonceDiskState {
                last_persisted_epoch: 0,
                next_allocated_block: 0,
            }
        };

        // Monotonic epoch rollback protection:
        // epoch = max(wall_clock, last_persisted_epoch + 1)
        let safe_epoch = if now_secs <= disk_state.last_persisted_epoch {
            disk_state.last_persisted_epoch.saturating_add(1)
        } else {
            now_secs
        };

        let start_counter = disk_state.next_allocated_block;
        let initial_block_limit = start_counter + NONCE_BLOCK_SIZE;

        let mgr = Self {
            state_file_path: path,
            current_epoch: safe_epoch,
            current_counter: start_counter,
            block_limit: initial_block_limit,
        };

        // Commit first reservation block with fsync before returning
        mgr.persist_block_reservation()?;

        Ok(mgr)
    }

    /// Persist the next block reservation to disk with atomic write and fsync.
    fn persist_block_reservation(&self) -> io::Result<()> {
        let disk_state = NonceDiskState {
            last_persisted_epoch: self.current_epoch,
            next_allocated_block: self.block_limit,
        };

        let json = serde_json::to_string_pretty(&disk_state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        let tmp_path = self.state_file_path.with_extension("tmp");
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp_path)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
        }

        fs::rename(&tmp_path, &self.state_file_path)?;

        // Fsync parent directory on Unix if possible
        #[cfg(unix)]
        if let Some(parent) = self.state_file_path.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }

        Ok(())
    }

    /// Allocate a message counter and epoch. If current block reservation is exhausted,
    /// atomically reserves the next block of 1,000 on disk with fsync before issuing.
    pub fn allocate_msg(&mut self) -> io::Result<(u32, u64)> {
        if self.current_counter >= self.block_limit {
            self.block_limit += NONCE_BLOCK_SIZE;
            self.persist_block_reservation()?;
        }

        let counter = self.current_counter;
        self.current_counter += 1;
        Ok((self.current_epoch, counter))
    }

    pub fn current_epoch(&self) -> u32 {
        self.current_epoch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nonce_manager_lifecycle_and_durability() {
        let tmp_dir = std::env::temp_dir().join("gibberish_test_nonce_1");
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).unwrap();
        let state_path = tmp_dir.join("nonce_state.json");

        {
            let mut mgr = NonceManager::new(Some(state_path.clone())).unwrap();
            let (epoch1, c0) = mgr.allocate_msg().unwrap();
            assert_eq!(c0, 0);
            assert!(epoch1 > 0);

            let (_epoch2, c1) = mgr.allocate_msg().unwrap();
            assert_eq!(c1, 1);
        }

        // Reopen NonceManager: should skip past the previous block of 1,000 to guarantee zero collisions
        {
            let mut mgr2 = NonceManager::new(Some(state_path.clone())).unwrap();
            let (_epoch, next_counter) = mgr2.allocate_msg().unwrap();
            assert_eq!(next_counter, 1000);
        }

        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_clock_rollback_protection() {
        let tmp_dir = std::env::temp_dir().join("gibberish_test_nonce_rollback");
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).unwrap();
        let state_path = tmp_dir.join("nonce_state.json");

        // Write an artificial state far in the future
        let future_epoch = 2_000_000_000u32;
        let future_state = NonceDiskState {
            last_persisted_epoch: future_epoch,
            next_allocated_block: 5000,
        };
        fs::write(&state_path, serde_json::to_string(&future_state).unwrap()).unwrap();

        let mgr = NonceManager::new(Some(state_path.clone())).unwrap();
        // Even if wall clock is earlier than future_epoch, safe_epoch must advance past it
        assert!(mgr.current_epoch() > future_epoch);

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
