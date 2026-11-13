# ADR 0003: Layered early configuration

Status: accepted for the early implementation

## Decision

Configuration precedence is CLI argument, environment, project configuration,
user configuration, then compiled default. Secrets are accepted only from the
environment or a future OS secret-store adapter; they are not deserialized from
project files.

Unknown fields and invalid permission profiles fail with a clear error. This
early format may change before the first tagged compatibility promise.
