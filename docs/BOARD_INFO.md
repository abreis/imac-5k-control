# XIAO ESP32C6 board info

Capture date: 2026-03-24

Device probed with:

- `espflash board-info`
- `espflash read-flash 0x8000 0x1000 /tmp/esp-partitions.bin`
- `espflash partition-table /tmp/esp-partitions.bin`
- `espflash partition-table --to-csv /tmp/esp-partitions.bin`

Board info at time of capture:

- Board: `Seeed Studio XIAO ESP32C6`
- Chip: `esp32c6` revision `v0.2`
- Crystal: `40 MHz`
- Flash size: `4MB`
- Features: `WiFi 6`, `BT 5`
- MAC: `98:a3:16:8d:f4:e4`

## RAM and flash specs

Confirmed from the `ESP32-C6` datasheet for this board:

- `HP SRAM`: `512 KB`
- `LP SRAM`: `16 KB`
- `L1 cache`: `32 KB`
- `ROM`: `320 KB`
- On-chip flash: `4MB`

Notes:

- The relevant main application RAM is `512 KB` of `HP SRAM`.
- `LP SRAM` is a separate low-power memory region and should not be treated as
  general application heap.

## Partition table

```csv
# ESP-IDF Partition Table
# Name,Type,SubType,Offset,Size,Flags
nvs,data,nvs,0x9000,0x6000,
phy_init,data,phy,0xf000,0x1000,
factory,app,factory,0x10000,0x3f0000,
```

## Human-readable view

- `nvs`: offset `0x9000`, size `0x6000` (24 KiB)
- `phy_init`: offset `0xf000`, size `0x1000` (4 KiB)
- `factory`: offset `0x10000`, size `0x3f0000` (4032 KiB)

## Notes

- This capture is for the specific `XIAO ESP32C6` board connected during the
  probe.
- Do not assume the same partition layout applies to other `ESP32-C6` boards,
  even if they use the same chip.
- The partition table was read directly from flash at address `0x8000`.
- This layout matches a simple ESP-IDF-style single-app image with no OTA
  slots.
- The observed flash layout fully occupies the available `4MB` on-chip flash.
