# oozextract (vendored)

This is a copy of the `oozextract` crate, version 0.5.5, by lvlvllvlvllvlvl
(<https://github.com/lvlvllvlvllvlvl/oozextract>, <https://crates.io/crates/oozextract>).
The crate declares the MIT license. It is a Rust port of `ooz` by powzix
(<https://github.com/powzix/ooz>).

Changes made here:

- Removed the command line tool, the tests, benches and the optional `tokio`, `wasm` and `clap` dependencies.
- `src/algorithm/kraken.rs` and `src/decoder/mod.rs` now read the "excess bytes" form of a
  Kraken table. The upstream crate refuses it with "excess bytes not supported". PS5 packages use it.
- `decode_bytes_type12` returned the wrong byte count for a table with one symbol (a block that repeats
  one byte). It now returns the bytes it read, as `ooz` does.
