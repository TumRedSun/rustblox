//! RustBlox desktop binary. All platform entry points live in the library
//! crate (`src/lib.rs`) so the same code runs on desktop and mobile.

fn main() {
    rustblox::run_desktop();
}
