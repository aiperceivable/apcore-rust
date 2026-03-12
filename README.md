# apcore

APCore SDK for Rust — AI Partner Core protocol implementation.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
apcore = "0.13"
```

## Quick Start

```rust
use apcore::client::APCore;

#[tokio::main]
async fn main() {
    let client = APCore::new();
    // TODO: usage example
}
```

## Documentation

- [Protocol Spec](https://github.com/aipartnerup/apcore-rust)

## License

Apache-2.0
