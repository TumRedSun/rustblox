//! iOS app executable entry: registers the platform hooks and runs the
//! client (winit's iOS backend handles the UIApplication lifecycle).

fn main() {
    rustblox_mobile::run_ios();
}
