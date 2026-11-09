//! Live smoke test of the auth pipeline against the real Roblox endpoint.
//!
//! Uses dummy credentials: the expected outcomes are
//!   * `Incorrect username or password` (401 code 1) — proves the CSRF
//!     rotation + cookie jar + error parsing all work, or
//!   * a captcha requirement (server-side bot detection on this IP).
//! Anything else indicates a pipeline bug.

use rustblox::roblox::auth;
use rustblox::roblox::client::RobloxClient;
use rustblox::roblox::errors::ApiError;

fn main() {
    let mut c = RobloxClient::new();
    println!("POST auth.roblox.com/v2/login (dummy credentials)...");
    match auth::login_password(&mut c, "definitelynotarealuser99", "wrongpass123") {
        Ok(_) => println!("UNEXPECTED SUCCESS (should not happen with dummy creds)"),
        Err(e @ ApiError::InvalidCredentials) => {
            println!("PIPELINE OK \u{2713}  server said: {e}")
        }
        Err(e @ ApiError::CaptchaRequired) => println!(
            "PIPELINE OK \u{2713}  captcha gate hit (expected from datacenter IPs): {e}"
        ),
        Err(e) => println!("PIPELINE REACHED SERVER, error: {e}"),
    }
    println!("csrf token cached: {}", c.csrf.is_some());
    println!("cookies in jar: {}", c.jar_len());
}
