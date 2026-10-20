//! Embed a Windows application manifest: modern common controls (v6),
//! per-monitor DPI awareness, long-path support and UTF-8 as the code page.
//! No resource compiler is needed.

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        use embed_manifest::{embed_manifest, new_manifest};
        embed_manifest(new_manifest("Calamansi")).expect("unable to embed manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
