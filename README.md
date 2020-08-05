c-lightning-http-plugin
=======================

This is a direct proxy for the unix domain socket from the HTTP interface

## Command line options

- `http-user`
    - user name to use for Basic Auth
    - default: `lightning`
- `http-pass`
    - password to use for Basic Auth
    - REQUIRED. All requests will be unauthorized if not supplied
- `http-port`
    - port to bind the rpc server to
    - default: `8080`
    
## Installation

Install `cargo`
```
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### From Source

```
git clone https://github.com/Start9Labs/c-lightning-http-plugin
cd c-lightning-http-plugin
cargo build --release
lightningd --plugin=/path/to/c-lightning-http-plugin/target/release/c-lightning-http-plugin
```

