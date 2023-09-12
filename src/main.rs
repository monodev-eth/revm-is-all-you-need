use anyhow::Result;
include!("revm_examples.rs");

#[tokio::main]
async fn main() -> Result<()> {
    println!("Hello, world!");
    Ok(())
}
