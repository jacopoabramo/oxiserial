use std::sync::OnceLock;

use tokio::runtime::{Builder, Runtime};

use crate::errors::SerialError;

struct Owned {
    pid: u32,
    runtime: Runtime,
}

static RUNTIME: OnceLock<Owned> = OnceLock::new();

/// The runtime that runs every port operation, started on first use.
pub fn runtime() -> Result<&'static Runtime, SerialError> {
    let owned = RUNTIME.get_or_init(|| Owned {
        pid: std::process::id(),
        runtime: Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("oxiserial")
            .enable_all()
            .build()
            .expect("failed to start the tokio runtime"),
    });
    // A forked child inherits the runtime's memory but not its threads.
    if owned.pid == std::process::id() {
        Ok(&owned.runtime)
    } else {
        Err(SerialError::Forked)
    }
}
