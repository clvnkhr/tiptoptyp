# Native terminal notices

libghostty-vt-sys 0.2.1 statically builds Ghostty commit
`a887df42c56f6de86c0fe6da9c4eeca37931e083`. Cargo's Rust license scan does not
cover its Zig/C++ inputs. `cargo xtask generate-notices` also includes these
notices in the packaged `THIRD_PARTY_NOTICES`:

- Ghostty: `LICENSE` from the pinned source tree, MIT.
- simdutf: version 9.0.0 in Ghostty's amalgamated header; MIT notice from
  <https://github.com/simdutf/simdutf/blob/v9.0.0/LICENSE-MIT>.
- Highway: commit `66486a10623fa0d72fe91260f96c892e41aceb06`, pinned by
  Ghostty's `pkg/highway/build.zig.zon`; its BSD-3-Clause option is used.
- uucode: 0.2.0, hash
  `uucode-0.2.0-ZZjBPqZVVABQepOqZHR7vV_NcaN-wats0IB6o-Exj6m9`;
  MIT notice from the downloaded Zig package.
- uucode's UTF-8 decoder and Unicode notices: the package's `LICENSE.md`
  references these but omits its `licenses/` directory from the archive.
  Retrieved from the upstream `master` license files at
  <https://github.com/jacobsandlund/uucode/tree/master/licenses> on 2026-09-22.

The Rust wrappers and portable-pty are covered by cargo-about's generated
section. Zig's bundled compiler runtime uses the MIT license from the Zig
0.15.2 distribution, retained here as well.
