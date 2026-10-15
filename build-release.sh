#!/bin/sh
# Release build: x86-64-v3, linked against glibc 2.31 so it runs on any distro from 2020 on
# (Ubuntu 20.04+, Debian 11+, Fedora 32+, Arch...). Needs cargo-zigbuild + zig:
#   pip install ziglang cargo-zigbuild      (or: cargo install cargo-zigbuild, and zig from ziglang.org)
# and the ALSA development files (Debian/Ubuntu: libasound2-dev, Fedora: alsa-lib-devel, Arch: alsa-lib).
# For a build just for your own machine, plain `cargo build --release` is enough.
set -e
cd "$(dirname "$0")"
# --allow-shlib-undefined: the build machine's libasound may need a newer glibc than 2.31; at run
# time the user's own libasound is used, which matches their glibc
RUSTFLAGS="-C target-cpu=x86-64-v3 -C link-arg=-Wl,--allow-shlib-undefined" \
  cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.31
v=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
out="dist/cratebox-$v-linux-x86_64-v3"
rm -rf "$out" && mkdir -p "$out"
cp target/x86_64-unknown-linux-gnu/release/cratebox "$out/"
strip "$out/cratebox" 2>/dev/null || true
cp packaging/install.sh packaging/cratebox.desktop packaging/cratebox.png README.md "$out/"
mkdir -p "$out/licenses" && cp assets/fonts/LICENSE-*.txt "$out/licenses/"
tar -C dist -czf "$out.tar.gz" "$(basename "$out")"
echo "$out.tar.gz"
