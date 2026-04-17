# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-04-17

### Added

- `Config::tls_handshake_timeout` bounds the time a TLS handshake may take
  before the connection is dropped. Defaults to 5 seconds.
- `Config::max_pending_connections` caps the number of in-flight TLS
  handshakes; new connections are dropped once the cap is reached.
  Defaults to 4096.
- `middleware::callback::RequestHandler` trait for observing the request
  body as it is polled by the inner service, with `on_body_chunk`,
  `on_end_of_stream`, and `on_body_error` hooks. A blanket no-op impl is
  provided for `()` so callers only interested in the response side can
  write `type RequestHandler = ();`.
- `middleware::callback::RequestBody<B, H>` wraps the inner service's
  request body and surfaces every frame event to the configured
  `RequestHandler`.
- `ResponseHandler::on_body_error` reports errors produced while polling
  the response body, separately from service-level errors.

### Changed

- **Breaking:** `MakeCallbackHandler` now produces a pair of handlers per
  request. The single `type Handler` and `make_handler(&Parts) ->
  Self::Handler` are replaced by `type RequestHandler`, `type
  ResponseHandler`, and `make_handler(&Parts) -> (Self::RequestHandler,
  Self::ResponseHandler)`.
- **Breaking:** `ResponseHandler::on_error` is renamed to
  `on_service_error` to distinguish service-future errors from
  response-body errors, which now flow through `on_body_error`.
- **Breaking:** Services wrapped by `CallbackLayer` now receive
  `Request<RequestBody<B, _>>` rather than `Request<B>`. Body-polymorphic
  inner services (e.g. `axum::Router`) are unaffected; monomorphic inner
  services that require a specific body type (e.g.
  `tonic::transport::Channel`) must rebox at the call site. See the
  module docs for an example.
- The fixed 1 second sleep on `accept()` errors is replaced with
  exponential backoff starting at 5 ms and capped at 1 second, resetting
  on success. Recovery from transient conditions such as `EMFILE` is now
  faster while still avoiding a spin loop.

### Removed

- **Breaking:** `Config::allow_insecure`, which peeked at the first byte
  of each connection to route plain-text traffic alongside TLS on the
  same listener. It was a temporary compatibility shim with no way to
  bound the resources consumed by slow or malicious TLS handshakes.
  Callers that need both plain-text and TLS should bind separate
  listeners.

## [0.1.0] - 2025-07-22

- Initial release.

[0.2.0]: https://github.com/mystenlabs/sui-http/releases/tag/v0.2.0
[0.1.0]: https://github.com/mystenlabs/sui-http/releases/tag/v0.1.0
