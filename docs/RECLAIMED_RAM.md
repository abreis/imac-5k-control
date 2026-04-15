# `esp_alloc::heap_allocator!` and reclaimed DRAM

Investigation date: 2026-03-24

Project versions examined:

- `esp-alloc = 0.9.0`
- `esp-hal = 1.0.0`
- `esp-hal-procmacros = 0.21.0`
- `esp-bootloader-esp-idf = 0.4.0`

## Summary

When used inside `esp_alloc::heap_allocator!`, both of the following place the
heap backing buffer into reclaimed DRAM:

```rust
#[unsafe(link_section = ".dram2_uninit")]
#[ram(reclaimed)]
```

The difference is that `#[ram(reclaimed)]` is the higher-level `esp-hal`
attribute. It expands to `#[unsafe(link_section = ".dram2_uninit")]` and also
adds compile-time validation that the item type is valid for reclaimed memory.

## What `heap_allocator!` actually does

`esp_alloc::heap_allocator!` accepts attributes and attaches them directly to a
hidden static buffer:

```rust
$(#[$m])*
static mut HEAP: core::mem::MaybeUninit<[u8; $size]> = core::mem::MaybeUninit::uninit();
```

It then registers that buffer as an internal-memory heap region.

Source:

- `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/esp-alloc-0.9.0/src/macros.rs`

Relevant lines:

- docs mention `heap_allocator!(#[ram(reclaimed)] size: 64000);`
- the macro forwards the attributes to the hidden `HEAP` static

## One: `#[unsafe(link_section = ".dram2_uninit")]`

This is the raw Rust/linker-level form.

Effect:

- puts the hidden heap buffer into linker section `.dram2_uninit`
- that section is mapped by `esp-hal`'s linker script into `dram2_seg`
- the section is declared `NOLOAD`, meaning startup code does not copy or zero
  it as normal initialized data

`esp-hal` linker snippet:

```ld
.dram2_uninit (NOLOAD) : ALIGN(4) {
    *(.dram2_uninit)
} > dram2_seg
```

Source:

- `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/esp-hal-1.0.0/ld/sections/dram2.x`

Meaning in practice:

- the heap memory comes from reclaimed DRAM that is otherwise unused after the
  ESP-IDF bootloader hands off to the app
- because it is uninitialized memory, contents must not be assumed valid on
  startup

## Two: `#[ram(reclaimed)]`

This is the `esp-hal` proc-macro form.

In `esp-hal-procmacros`, `reclaimed`:

- sets the internal `dram2_uninit` flag
- selects section name `.dram2_uninit`
- emits `#[unsafe(link_section = ".dram2_uninit")]`
- adds a compile-time trait assertion requiring the static's type to implement
  `esp_hal::Uninit`

Source:

- `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/esp-hal-procmacros-0.21.0/src/ram.rs`

Important behavior:

- `ram(reclaimed)` is only accepted when `esp-hal-procmacros` is built with
  `__esp_idf_bootloader`; otherwise it emits:

```text
`ram(reclaimed)` requires the esp-idf bootloader
```

## Type safety check added by `#[ram(reclaimed)]`

`esp-hal` defines a hidden marker trait:

```rust
pub unsafe trait Uninit: Sized {}
```

Implemented for:

```rust
core::mem::MaybeUninit<T>
[core::mem::MaybeUninit<T>; N]
```

So `#[ram(reclaimed)]` enforces that the item is some `MaybeUninit` form, which
matches the bootloader requirement for reclaimed DRAM.

Source:

- `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/esp-hal-1.0.0/src/lib.rs`

## Why this works with `heap_allocator!`

`heap_allocator!` creates:

```rust
static mut HEAP: MaybeUninit<[u8; SIZE]> = MaybeUninit::uninit();
```

That type satisfies `esp_hal::Uninit`, so `#[ram(reclaimed)]` is valid here.

## ESP32-C6 RAM numbers in this context

For the specific `XIAO ESP32C6` board discussed here, the chip datasheet gives:

- `HP SRAM`: `512 KB`
- `LP SRAM`: `16 KB`
- `L1 cache`: `32 KB`
- `ROM`: `320 KB`

For `heap_allocator!`, the relevant memory is the `512 KB` of `HP SRAM`.

That does not mean the application sees one flat `512 KB` heap-capable region.
In the `esp-hal` linker layout for `esp32c6`, this memory is split into:

- `RAM`: `0x6E610` = `452,112` bytes
- `dram2_seg`: `65,536` bytes

This totals `517,648` bytes (`0x7E610`), which is slightly less than the full
`512 KiB` physical HP SRAM. The difference comes from memory reserved by the
platform and linker layout. The reclaimed heap placement discussed in this note
uses the `dram2_seg` portion via `.dram2_uninit`.

## Practical comparison

### `#[unsafe(link_section = ".dram2_uninit")]`

Pros:

- direct and explicit
- does not depend on the `ram` proc macro

Cons:

- no compile-time type validation
- easier to misuse on an initialized type
- encodes linker-section knowledge directly at the call site

### `#[ram(reclaimed)]`

Pros:

- expands to the correct linker section automatically
- checks that the type is appropriate for reclaimed memory
- documents intent better than a raw section name

Cons:

- requires the ESP-IDF bootloader support expected by `esp-hal`

## Bottom line

For `esp_alloc::heap_allocator!`:

- `#[unsafe(link_section = ".dram2_uninit")]` means "put the heap buffer into
  reclaimed, uninitialized DRAM"
- `#[ram(reclaimed)]` means the same placement, but with `esp-hal`-specific
  validation and a clearer semantic name

If both are available in the build, `#[ram(reclaimed)]` is the better spelling.
