use std::sync::OnceLock;

use tokio::runtime::{Builder, Runtime};

use crate::errors::SerialError;

struct Owned {
    pid: u32,
    runtime: Runtime,
}

static RUNTIME: OnceLock<Result<Owned, SerialError>> = OnceLock::new();

fn start() -> Result<Owned, SerialError> {
    let runtime = Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("oxiserial")
        .enable_all()
        .build()
        .map_err(|err| SerialError::Os {
            code: err.raw_os_error(),
            message: format!("failed to start the tokio runtime: {err}"),
        })?;
    Ok(Owned {
        pid: std::process::id(),
        runtime,
    })
}

/// Whether this is a forked child of the process that started the runtime.
///
/// The child must not wake the runtime's tasks: that wakes its I/O driver, whose
/// descriptors the child may not have, as with kqueue on macOS.
pub fn forked() -> bool {
    matches!(RUNTIME.get(), Some(Ok(owned)) if owned.pid != std::process::id())
}

/// The runtime that runs every port operation, started on first use.
pub fn runtime() -> Result<&'static Runtime, SerialError> {
    let owned = RUNTIME.get_or_init(start).as_ref().map_err(Clone::clone)?;
    // A forked child inherits the runtime's memory but not its threads.
    if owned.pid == std::process::id() {
        Ok(&owned.runtime)
    } else {
        Err(SerialError::Forked)
    }
}
