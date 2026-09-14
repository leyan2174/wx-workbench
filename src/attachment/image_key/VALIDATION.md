# Image Validation Boundaries

`validate_existing_evidence_for_db_dir` distinguishes verified AES evidence,
no usable evidence, and contradictory evidence. Migration accepts no evidence
only with `--allow-unverified`. Contradictory evidence always stops migration,
including when that flag is supplied. I/O and validation-budget errors remain
errors. AES header evidence does not independently verify XOR material.

Web image decoding obtains `Snapshot::image_material()` from the authorized
account's encrypted Store and passes it in memory. It does not create a
plaintext key-file bridge. Missing, legacy-only, and corrupt stores are not
silently replaced by configuration keys. XOR-only snapshots retain their XOR.

Direct MCP plaintext `image_key_file` overrides are unsupported and rejected.
Model-supplied paths remain stripped by host policy; they grant no file access.

CLI image batch decoding with both input and output paths explicitly supplied
also supports an absent configuration file. In that case legacy XOR and V1
decoding need no account access, while V2 still requires explicit AES material.
An existing configuration continues to require its encrypted Store; missing or
corrupt stores and legacy-only configurations never trigger this standalone path.
