//! Binary entry point. CLI wiring lands in a later task; for now this only
//! proves the crate links.

fn main() -> anyhow::Result<()> {
    println!("gribovik {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}
