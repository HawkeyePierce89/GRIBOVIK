//! Build script: guard the one precondition the crate cannot check at runtime.
//!
//! `rust-embed` bakes `web/dist` into the binary, and when that directory is
//! missing it embeds nothing at all — producing a `gribovik` that starts,
//! serves a blank page, and gives no hint why. Failing the build instead turns
//! a confusing runtime symptom into a one-line instruction.

use std::path::Path;

fn main() {
    // Only the built SPA matters here; its sources are the frontend build's
    // problem, and watching them would rebuild the crate on every edit.
    println!("cargo:rerun-if-changed=web/dist");

    if !Path::new("web/dist/index.html").exists() {
        println!(
            "cargo:warning=web/dist/index.html is missing — run `just build-web` first \
             (or `npm ci && npm run build` in web/)"
        );
        panic!("missing frontend build: web/dist/index.html — run `just build-web` first");
    }
}
