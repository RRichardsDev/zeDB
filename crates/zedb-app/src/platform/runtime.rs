//! Shared tokio runtime. GPUI has its own executor, but reqwest futures
//! need a tokio reactor; we spawn them here and await the JoinHandle from
//! GPUI tasks (JoinHandle is executor-agnostic).

use std::sync::OnceLock;

use tokio::runtime::Runtime;

pub fn tokio() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().expect("start tokio runtime"))
}

/// Drop a potentially huge value off the UI thread. Freeing a
/// multi-gigabyte result set is millions of tiny deallocations; done
/// synchronously it beachballs the app for minutes.
pub fn drop_in_background<T: Send + 'static>(value: T) {
    tokio().spawn(async move {
        drop(value);
    });
}
