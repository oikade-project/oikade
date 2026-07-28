//! Transactional state for the Oikade runtime.

mod device_state;
mod store;

pub use device_state::{DeviceStateStore, MAX_DEVICE_STATE_SIZE};
pub use store::{Bucket, Namespace, Storage, StorageError};
