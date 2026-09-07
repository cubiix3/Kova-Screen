# Third-party notices

Kova Screen itself is licensed under [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE) at your option. It builds on open-source crates
whose own licenses are listed here.

Regenerate this summary with:

```bash
cargo install cargo-license
cargo license --avoid-build-deps --avoid-dev-deps
```

## Summary

Counts are for the dependency graph as resolved by `Cargo.lock`, which covers
every platform Tauri supports rather than only the one Kova Screen ships.

| License | Crates |
| --- | ---: |
| `Apache-2.0 OR MIT` | 318 |
| `MIT` | 118 |
| `Apache-2.0 OR MIT OR Zlib` | 23 |
| `Unicode-3.0` | 18 |
| `MIT OR Unlicense` | 12 |
| `MPL-2.0` | 5 |
| `Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT` | 5 |
| `ISC` | 3 |
| `BSD-3-Clause` | 3 |
| `Zlib` | 2 |
| `Apache-2.0` | 2 |
| `Apache-2.0 OR LGPL-2.1-or-later OR MIT` | 2 |
| `Apache-2.0 OR BSD-3-Clause` | 2 |
| `Apache-2.0 OR BSD-3-Clause OR MIT` | 2 |
| `Apache-2.0 OR BSD-2-Clause OR MIT` | 2 |
| `CDLA-Permissive-2.0` | 1 |
| `BSD-3-Clause OR MIT` | 1 |
| `BSD-3-Clause AND MIT` | 1 |
| `Apache-2.0 OR ISC OR MIT` | 1 |
| `Apache-2.0 OR CC0-1.0 OR MIT-0` | 1 |
| `Apache-2.0 AND MIT` | 1 |
| `Apache-2.0 AND ISC` | 1 |
| `0BSD OR Apache-2.0 OR MIT` | 1 |
| `(Apache-2.0 OR MIT) AND Unicode-3.0` | 1 |

Every license above is permissive or weak file-level copyleft. There is no
GPL, LGPL-only or AGPL dependency, and nothing that would restrict how Kova
Screen may be used or redistributed.

Where a crate offers a choice (`Apache-2.0 OR MIT`), Kova Screen relies on it
under terms compatible with its own dual MIT/Apache-2.0 licensing.

## Mozilla Public License 2.0 components

Five crates are MPL-2.0, and all of them are compiled into the shipped Windows
binary. They arrive through Tauri:

| Crate | Reached via |
| --- | --- |
| `cssparser` | Tauri's HTML and CSP processing |
| `cssparser-macros` | as above |
| `selectors` | as above |
| `dtoa-short` | as above |
| `option-ext` | `dirs`, for locating standard directories |

MPL-2.0 is file-level copyleft. It permits distributing a larger work — this
application — under different terms, provided the MPL-licensed files themselves
remain under the MPL and their source is available to recipients.

Kova Screen uses all five **unmodified**, exactly as published. Their complete
corresponding source is available on crates.io and via the links below, and the
exact versions used in any given build are recorded in `Cargo.lock`:

- <https://crates.io/crates/cssparser>
- <https://crates.io/crates/cssparser-macros>
- <https://crates.io/crates/selectors>
- <https://crates.io/crates/dtoa-short>
- <https://crates.io/crates/option-ext>

Should a future change modify any of these files, that modified source must be
published under the MPL-2.0 as well.

## Notable direct dependencies

| Crate | License | Used for |
| --- | --- | --- |
| `tauri` | `Apache-2.0 OR MIT` | Application shell, tray, WebView windows |
| `windows`, `windows-core` | `Apache-2.0 OR MIT` | Win32 and WinRT bindings |
| `image` | `Apache-2.0 OR MIT` | PNG, JPEG and WebP encoding |
| `gif` | `Apache-2.0 OR MIT` | GIF encoding |
| `color_quant` | `MIT` | GIF palette quantisation |
| `rusqlite`, `libsqlite3-sys` | `MIT` | Capture history |
| `ureq` | `Apache-2.0 OR MIT` | HTTP client for uploads |
| `rustls`, `rustls-webpki` | `Apache-2.0 OR ISC OR MIT`, `ISC` | TLS |
| `webpki-roots` | `CDLA-Permissive-2.0` | Root certificates |
| `serde`, `serde_json` | `Apache-2.0 OR MIT` | Settings serialisation |
| `tracing` | `MIT` | Logging |
| `parking_lot` | `Apache-2.0 OR MIT` | Synchronisation |
| `time` | `Apache-2.0 OR MIT` | Timestamps for filenames |

SQLite itself, vendored by `libsqlite3-sys`, is in the public domain.

## Attribution

Flameshot and ShareX were a functional inspiration for Kova Screen. No code
from either project is used, and neither is a dependency.
