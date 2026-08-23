# `solum-backup` protocol v1 fixture

This directory contains synthetic, public test material only. It contains no
real user, account, device, or personal content.

- Payloads: `payload.json` is the original `solum-export` v2 compatibility
  artifact; `payload-v3.json` is the synthetic v3 `_portable` contract. Both
  are normalized recursively by lexicographic object-key order and compact
  JSON encoding before compression.
- Master key (hex):
  `000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f`
- Nonce (hex, fixture only; never reuse in production):
  `a0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7`
- User UUID: `11111111-1111-4111-8111-111111111111`
- Device generation: `7`
- Backup UUID: `22222222-2222-4222-8222-222222222222`
- Created at: `2026-01-02T03:04:05.000Z`
- Encrypted-body digest: `sha256` over normalized `payload.json`; the digest is
  not exposed in the clear header.
- Exact v2 container bytes: whitespace-insensitive hex in `container.hex`.
- v2 container length: `647` bytes; SHA-256:
  `8fa510054eb5df42096830b5436510fa2d7062e6d5a362ae2b7dea203e52be5b`
- Exact v3 container bytes: whitespace-insensitive hex in `container-v3.hex`.
- v3 container length: `880` bytes; SHA-256:
  `dc010cfb93432f01e2903b584161330f603e42a83f817010567a6d4b59935d4e`

The fixed nonce exists solely to make the compatibility artifact byte-stable.
Production writers must always obtain a fresh 24-byte nonce from a secure OS
random source.
