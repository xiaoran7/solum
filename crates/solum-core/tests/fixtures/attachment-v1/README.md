# `solum-attachment` protocol v1 fixed fixture

This directory contains only synthetic public test data. It is a byte-level
interoperability contract for Android, iOS, and HarmonyOS implementations; it
must never contain user data or production keys.

- plaintext UTF-8: `Synthetic attachment fixture bytes only.`
- master key: bytes `00 01 02 ... 1f`
- user id: `11111111-1111-4111-8111-111111111111`
- device generation: `7`
- backup id: `22222222-2222-4222-8222-222222222222`
- attachment id: `document-public-fixture`
- plaintext bytes: `40`
- plaintext SHA-256: `920f12415723f81b596e66aadf520f4568e118b490e88e9ffb0572cbfd58b21f`
- fixture chunk size: `16` bytes (production writers use `1048576`)
- chunk nonces: bytes `00..17`, `18..2f`, and `30..47`
- `container.hex`: lowercase hex of the exact encoded container; whitespace is ignored
- container bytes: `667`
- container SHA-256: `b0e113feb33d49dd9e20aecdad0793b2e98d7780d6967430d2537695e94864f3`

The plaintext byte length and SHA-256 are authenticated through the hidden
binding described in `docs/ARCHITECTURE.md`; the binding itself is not present
in the clear container.
