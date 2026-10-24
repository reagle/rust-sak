//! Scratch example for playing with the `fetch` module.
//!
//! Run with: `cargo run --example playground --features fetch`

use rust_sak::fetch::{Fetch, FetchError};

#[tokio::main]
async fn main() -> Result<(), FetchError> {
    let fetch = Fetch::new();

    let body = fetch
        .header("User-Agent", "rust-sak-playground")
        .retries(2)
        .text("https://vinicius.io")
        .await?;

    println!("{body}");

    Ok(())
}
