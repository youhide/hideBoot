# hideBoot

A UEFI boot manager for Linux, written in Rust, with boot counting and
automatic fallback.

**Works, in QEMU.** It boots hideOS's UKIs under OVMF, counts attempts by
renaming in place, and passes over an image the firmware refuses. hideOS
replaces `systemd-boot` with it in milestone H7; both follow the same
on-disk convention.

Build it with a Rust that has the UEFI target:

```sh
cargo build --release -p hideboot --target x86_64-unknown-uefi
```

`crates/hideboot-core` holds the decisions — parsing names, the boot order,
the counter after an attempt — and is tested on the host with `cargo test`.

## What it does

1. Lists the Unified Kernel Images in `\EFI\Linux\` on the ESP.
2. Orders them newest first by version in the file name.
3. Skips any whose boot counter is exhausted.
4. Decrements the counter of the one it picks — by renaming the file — and
   boots it.
5. Shows a menu when a key is held at startup, and never otherwise.
6. Sets `LoaderBootCountPath` and `LoaderEntrySelected`, systemd-boot's
   variables, so the OS knows which file to rename when it marks the boot
   good.

That is all. No filesystem drivers beyond FAT, no configuration language, no
theming, no kernel loading other than a signed UKI through the firmware's own
`LoadImage`.

## The contract

hideBoot knows nothing about hideOS. It reads and writes one convention, from
the [Boot Loader Specification](https://uapi-group.org/specifications/specs/boot_loader_specification/)
and [Automatic Boot Assessment](https://uapi-group.org/specifications/specs/boot_loader_specification/#boot-counting):

```
\EFI\Linux\<name>-<version>+<left>-<done>.efi
```

- `+<left>` — boots remaining before this entry is considered bad.
- `-<done>` — boots already attempted.
- No `+` suffix — the entry has been marked good by the OS and is not counted.
- `+0-<done>` — exhausted; skipped, and the next newest entry boots instead.

The OS marks an entry good by renaming it without the suffix once it has
reached a working state. That rename is the OS's job, not the boot manager's.
Any system that follows the convention can use hideBoot, and hideOS can switch
between it and `systemd-boot` without changing anything on disk.

## Rules

The same as every boot-path crate in hideOS and oxinit: no `unwrap`, `expect`,
`panic!` or slice indexing; errors as values; `unsafe` confined to one module
with a `// SAFETY:` comment per block. A boot manager that panics is a machine
that does not boot.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
