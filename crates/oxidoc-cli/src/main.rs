//! `oxidoc` — command-line interface.
//!
//! Scaffolding only: argument parsing and the reader → writer pipeline land in slice 1.

fn main() {
    println!(
        "oxidoc {} — scaffolding; no conversions implemented yet (see docs/PORTING.md)",
        env!("CARGO_PKG_VERSION")
    );
}
