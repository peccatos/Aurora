# ADR 0001: Audio output abstraction

Status: Accepted

Use CPAL as the initial cross-platform output API behind an Aurora-owned
`AudioOutputBackend` boundary. Do not expose CPAL types in domain contracts.

Reason: the application needs a portable baseline while retaining the option
to add platform-specific output backends and modes.

Consequence: CPAL does not, by itself, provide a bit-perfect guarantee.
Platform-specific capabilities and verification must be implemented separately.
