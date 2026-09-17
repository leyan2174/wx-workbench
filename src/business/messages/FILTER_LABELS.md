# Message filter labels and legacy precision

`FilterLabel` expresses label intent using the existing `Kind` where it fits.
Sticker and Location are narrow independent labels because Kind does not
classify them. Link and File remain separate label intents, not aliases for
the semantic Structured kind.

`service/message_filter.rs` owns transport spelling profiles and the numeric
projection. CLI remains lowercase-only with its original ten labels. MCP uses
the same formal business spellings, while retaining its existing whitespace
trimming and ASCII case folding. The removed compatibility words
`namecard`/`app`/`emoji`/`voip` are rejected. CLI value parsers are unchanged.

The existing numeric wire cannot express link versus file: both project to 49.
This is explicit compatibility loss, not a claim that these business intents
are equivalent. No numeric storage field has been renamed as a business kind.
Raw numeric requests remain unchanged, including packed and negative selectors.

The WeChat read adapter continues to own the storage rule:

- Unsigned 32-bit selectors compare the low 32 bits.
- Other selectors compare the full signed 64-bit value.
- Legacy 43 excludes 62; semantic Kind::Video includes both.
- Legacy 10000 excludes 10002; semantic Kind::System includes both.
- Legacy 49 includes all app subtypes, while a packed selector matches exactly.
- Semantic and legacy filters, if both supplied, remain an intersection.

Daemon history and search intentionally leave `Filter.kinds` empty when
forwarding numeric requests through `LegacyReadPolicy`. Converting those
numbers to Kind would widen results. Search's special decoded app-message
path for the exact legacy selector list [49] is unchanged.

Tests cover canonical business labels, explicit rejection of unsupported MCP
compatibility words, CLI/MCP case differences, real MCP routing, and real synthetic SQLite selection for
base/packed/negative selectors and semantic versus legacy differences.
Standalone protocol, voice-host and image-security fixtures register the
production compatibility module. Execution requirements are in the
[test guide](../../../tests/README.md); fixtures use no real account.
