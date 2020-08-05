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

