//! The `zedb` binary: everything lives in the zedb-app library so
//! tests (and future binaries) can link against it.

fn main() {
    zedb_app::run();
}
