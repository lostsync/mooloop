//! The scan child for this crate's own tests (`tests/scan.rs`).
//!
//! The app runs its own binary as the child (`mooloop --scan-plugin <path>`,
//! `crates/mooloop-app/src/main.rs`). A test in this crate cannot launch that
//! binary, so it launches this one, which does the same thing through the same
//! function. It is never packaged.

fn main() {
    std::process::exit(
        mooloop_plugin_host::scan::run_child_from_args()
            .unwrap_or(mooloop_plugin_host::scan::EXIT_USAGE),
    );
}
