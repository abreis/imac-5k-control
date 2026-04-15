# `esp_alloc::heap_allocator!` plan for XIAO ESP32C6

Plan date: 2026-03-24

Related notes:

- `RECLAIMED_RAM.md`
- `BOARD_INFO.md`

## Goal

Use `esp_alloc::heap_allocator!` in a way that:

- makes use of reclaimed RAM first
- avoids eating too far into normal app RAM
- preserves enough headroom for stack growth and runtime allocations

## Key facts

For the `XIAO ESP32C6`, the relevant memory picture is:

- physical `HP SRAM`: `512 KB`
- `esp-hal` regular app `RAM`: `452,112` bytes
- reclaimed `dram2_seg`: `65,536` bytes

The reclaimed region is separate from the regular app RAM region and is the
best place to put heap first when possible.

## Important design point

`esp_alloc` supports multiple heap regions, and it is valid to call
`esp_alloc::heap_allocator!` more than once. If more than one suitable region
exists, allocation is attempted from the first added region first.

That means the preferred setup is:

```rust
esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
esp_alloc::heap_allocator!(size: 128 * 1024);
```

This gives:

- reclaimed RAM first
- regular RAM second
- a total heap of `192 KB`

## Why two heap regions are better

Using reclaimed RAM first is attractive because:

- it does not compete with the normal main app stack in regular RAM
- it makes use of memory that would otherwise go unused after the ESP-IDF
  bootloader hands off to the app

Using regular RAM for heap is still useful, but it needs more caution because
that region also needs to leave enough space for:

- the main stack
- static `.data` and `.bss`
- async task state
- runtime allocations from `esp-rtos`, `esp-radio`, Wi-Fi, and related code

## What the current build shows

The current release build uses:

```rust
esp_alloc::heap_allocator!(size: 128 * 1024);
```

From the generated linker map:

- current heap buffer in `.bss`: `128 KiB`
- remaining main stack space in regular RAM: about `220,792` bytes
  (`~215.6 KiB`)

This means the current `128 KiB` regular-RAM heap is conservative for this
firmware.

## Reasonable starting budgets

These are sensible heap configurations to try, in order:

### Option 1

```rust
esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
```

Use this if heap demand is small and you want the safest setup.

### Option 2

```rust
esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
esp_alloc::heap_allocator!(size: 96 * 1024);
```

Very conservative and likely safe.

### Option 3

```rust
esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
esp_alloc::heap_allocator!(size: 128 * 1024);
```

This is the recommended first serious target.

### Option 4

```rust
esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
esp_alloc::heap_allocator!(size: 160 * 1024);
```

Likely still fine, but should be tested and measured before treating it as the
default.

## Do we need to know stack size?

Yes, for the regular-RAM heap.

The regular heap lives in the same broad memory region that must also leave
room for the main stack, so increasing the regular heap reduces stack headroom.

However, stack size is not the whole story in this project:

- many large `async` locals become static task state, not stack
- some runtime components allocate internal-memory stacks dynamically

So there are really three separate memory budgets to watch:

- static and task-state RAM
- stack headroom
- heap usage

## Practical recommendation

The best practical approach is:

1. add reclaimed heap first
2. keep a moderate regular-RAM heap
3. measure heap usage under realistic runtime load
4. only increase the regular-RAM heap in small steps

Recommended first configuration:

```rust
esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
esp_alloc::heap_allocator!(size: 128 * 1024);
```

## What to measure next

After switching to the two-region setup, check:

- `esp_alloc::HEAP.stats()` right after boot
- `esp_alloc::HEAP.stats()` after Wi-Fi is connected
- `esp_alloc::HEAP.stats()` after MQTT and normal runtime activity are active

If heap usage stays comfortably below the total, stop there.

If heap usage is tight, raise the regular region gradually, for example from
`128 KiB` to `160 KiB`, and rebuild while checking the linker map again.

## Bottom line

The simplest good answer is:

- yes, use two calls if you want both regular RAM and reclaimed RAM in the heap
- yes, stack headroom matters for the regular-RAM heap
- no, you do not need to maximize the regular-RAM heap immediately
- the best first target for this firmware is `64 KiB reclaimed + 128 KiB regular`
