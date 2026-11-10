# ADR 0002: Bit-perfect claims

Status: Accepted

Bit-perfect is a verified property of the complete signal path, not a source
file property or a UI toggle.

The application reports eligibility separately from verification. If the OS
mixer, driver, backend or hardware path cannot be verified, status is
`Unverified`.
