# GhostChat patch of libsqlite3-sys 0.30.1

Why: RUSTSEC-2026-0459 / CVE-2026-119375. SQLite before 3.54.0 can return
stale page-cache memory when it opens a database together with a crafted
`-wal` file. `libsqlite3-sys` fixed it in 0.39.0, but `sqlx` 0.8 (used by
`crates/server` and `crates/desktop`) only accepts `libsqlite3-sys` ^0.30, and
no `sqlx` release accepts 0.39. So this directory is upstream
`libsqlite3-sys` 0.30.1 with its bundled C sources replaced by fixed ones. The
workspace `Cargo.toml` uses it through
`[patch.crates-io] libsqlite3-sys = { path = "vendor/libsqlite3-sys" }`.

## Upstream base

`libsqlite3-sys` 0.30.1 from crates.io (crate sha256
`2e99fb7a497b1e3339bc746195567ed8d3e24945ecd636e3619d20b9de9e9149`, the
checksum Cargo.lock had; rusqlite git `499cc7bb986e04cc66e6ed762522f7ea449178d1`),
copied unchanged from `~/.cargo/registry/src` except for the files below.

## Replaced files

Plain SQLite (`bundled`, what every build uses): 3.46.0 -> **3.54.1**
(`SQLITE_SOURCE_ID` 2026-10-16 14:45:20 555e07ef...e6ca76), taken from
`libsqlite3-sys` 0.39.1 on crates.io (crate sha256
`14a5f01b87b92705d07b433c06f96cbc764637d81a0bffb5734ce17cf0f9a2b3`), directory
`sqlite3/`.

| File | 0.30.1 sha256 (removed) | new sha256 |
| --- | --- | --- |
| `sqlite3/sqlite3.c` | `c01235302fe80da901fb70c7622c39147e29d9f29b7f6eb746b23517f320c90d` | `0c657666d819f261cc4007526ad57e1669de9949df7a8ea847aa284a2bf0ee52` |
| `sqlite3/sqlite3.h` | `d088aa96aa70db50f02acc5c86eca61a5d17556e4c363b9c06079239bf7f87b1` | `d6c84557261893439b341d2e6c35187ae3ea24fbfd843ad94616f72d89242b8c` |
| `sqlite3/sqlite3ext.h` | `b184dd1586d935133d37ad76fa353faf0a1021ff2fdedeedcc3498fff74bbb94` | `48963abfefb41ac329ef9d706acb5eadba952a99a748285c9ded6b5e0942721b` |

SQLCipher (`bundled-sqlcipher`, only the optional desktop feature
`sqlcipher-vault`, which no release build enables): SQLCipher 4.5.7 on SQLite
3.45.3 (affected) -> **SQLCipher 4.20.0 on SQLite 3.54.1**, taken from
`sqlcipher-src` 420.0.0+sqlcipher-4.20.0-sqlite-3.54.1-libtomcrypt-1.18.2 on
crates.io (crate sha256
`fd6b5280ed2593d07b2816ec531821c15c306903bcca224716f1fbcc2bbfedfe`; the crate
upstream `libsqlite3-sys` 0.40 uses), directory `sqlcipher/`. Its libtomcrypt
copy is not used: this build keeps OpenSSL as the crypto provider, as before.

| File | 0.30.1 sha256 (removed) | new sha256 | source file |
| --- | --- | --- | --- |
| `sqlcipher/sqlite3.c` | `943812115199728aee17b9c0a740b692be9337b5e59693333158bf4755df18fe` | `8cd2347d18886fee4651153fabeeeffb7cec008700987dab1c00261febc4220f` | `sqlcipher/sqlcipher.c` |
| `sqlcipher/sqlite3.h` | `6ca976709285f1410025d68feaf0237d8f87df9021cdb069c4dd9cf57c69972f` | `e36eb6ed4de77e56a633866a8f84ed339785662d0cf79cf099f420b638bf28ec` | `sqlcipher/sqlite3.h` |
| `sqlcipher/sqlite3ext.h` | `b184dd1586d935133d37ad76fa353faf0a1021ff2fdedeedcc3498fff74bbb94` | `48963abfefb41ac329ef9d706acb5eadba952a99a748285c9ded6b5e0942721b` | `sqlcipher/sqlite3ext.h` |
| `sqlcipher/LICENSE` | `ea4fcb309f14a22065e1ea45362d494d320012249ed865fe9c7c0946db754131` | `2a2826f6acf46fa650730cf42cbb22a642be33a7ef119c9c4f4bf6daf3bef48e` | `sqlcipher/LICENSE-sqlcipher` |

## Other edits

- `build.rs`, `bundled-sqlcipher` branch: adds
  `-DSQLITE_EXTRA_INIT=sqlcipher_extra_init`,
  `-DSQLITE_EXTRA_SHUTDOWN=sqlcipher_extra_shutdown` and `-DHAVE_STDINT_H=1`.
  SQLCipher 4.20 refuses to compile without the first two; the flags are the
  ones upstream `libsqlite3-sys` 0.39/0.40 pass. The plain SQLite branch is
  unchanged.
- `sqlite3/bindgen_bundled_version.rs`, `sqlite3/bindgen_bundled_version_ext.rs`
  and `sqlcipher/bindgen_bundled_version.rs`: only the `SQLITE_VERSION`,
  `SQLITE_VERSION_NUMBER` and `SQLITE_SOURCE_ID` constants are updated to the
  new headers' values. The rest of the 0.30.1 prebuilt bindings is kept:
  SQLite keeps its C ABI within 3.x, and comparing the 0.30.1 bindings with
  the 0.39.1 ones (generated from 3.54.1) shows the only struct change is
  fields appended to `Fts5ExtensionApi` (append-only, safe). The one 0.30.1
  binding without a 3.54.1 declaration is `sqlite3changeset_upgrade` (session
  extension, not compiled in; nothing calls it).
- `.cargo-ok` (registry marker) removed.

## Proof

- `crates/server/tests/sqlite_version.rs`,
  `linked_sqlite_has_wal_stale_memory_fix`: opens the server database through
  `ghostchat_server::open_database` (sqlx, WAL mode) and asserts
  `select sqlite_version()` >= 3.54.0. It failed with 3.46.0 before this patch.
- `crates/desktop/src/secure_vault.rs`, `vault_sqlcipher_has_wal_stale_memory_fix`
  (only with `--features sqlcipher-vault`): the same check on an SQLCipher
  vault.
- `cargo audit` no longer reports RUSTSEC-2026-0459: a `[patch]` path package
  has no crates.io source in Cargo.lock, so the advisory does not match it.
  There is deliberately no ignore entry in `.cargo/audit.toml`; if the patch
  is dropped while `libsqlite3-sys` is still < 0.39, the audit fails again.

## Dropping the patch

Once a `sqlx` release accepts `libsqlite3-sys` >= 0.39 (bundling SQLite
>= 3.54.0):

1. Upgrade `sqlx` (and `libsqlite3-sys` in `crates/desktop/Cargo.toml`, used
   by `sqlcipher-vault`; check that its `bundled-sqlcipher` SQLCipher is built
   on SQLite >= 3.54.0, e.g. `sqlcipher-src` >= 420.0.0).
2. Delete the `[patch.crates-io]` section and `exclude = ["vendor"]` from the
   workspace `Cargo.toml`, delete `vendor/libsqlite3-sys/`, and run
   `cargo update -p libsqlite3-sys`.
3. Keep both tests above; they must stay green. Update the "Dependency audit"
   section of `docs/SECURITY.md` and the note in `.cargo/audit.toml`.
4. `cargo audit` must still exit 0.

When re-vendoring a different SQLite instead, repeat the replacement above,
update the binding constants and this file's hashes, and run the checks with
`cargo check --target x86_64-pc-windows-gnu` (the C must build with mingw).
