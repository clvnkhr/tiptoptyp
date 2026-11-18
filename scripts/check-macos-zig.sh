#!/bin/sh
# Detect Zig/SDK linker incompatibility before the expensive Rust build.
# Use an isolated cache so a previous SDK's successful binary cannot mask it.
set -eu

sw_vers
xcode-select --print-path
xcrun --sdk macosx --show-sdk-path
xcrun --sdk macosx --show-sdk-version
zig version

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/ttt-zig-sdk.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT HUP INT TERM

cat > "$work_dir/build.zig" <<'ZIG'
const std = @import("std");

pub fn build(b: *std.Build) void {
    const executable = b.addExecutable(.{
        .name = "zig-sdk-check",
        .root_module = b.createModule(.{
            .root_source_file = b.path("main.zig"),
            .target = b.graph.host,
            .optimize = .ReleaseSafe,
            .link_libc = true,
        }),
    });
    const run = b.addRunArtifact(executable);
    b.default_step.dependOn(&run.step);
}
ZIG
cat > "$work_dir/main.zig" <<'ZIG'
extern "c" fn puts(message: [*:0]const u8) c_int;

pub fn main() void {
    if (puts("Zig libc link check passed") < 0) @panic("libc output failed");
}
ZIG

zig build --build-file "$work_dir/build.zig" --cache-dir "$work_dir/local-cache" \
    --global-cache-dir "$work_dir/global-cache" --summary all
