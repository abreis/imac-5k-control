# Display Board Power State Machine

Capture date: 2026-04-15

## Summary

This document turns the observed display-board power behavior into a normalized
state machine that future firmware can use as its source of truth.

The observations came from three experiments:

- no video cable connected
- video cable connected for the whole sequence
- video cable connected while the board was already in red-LED sleep

The original experiment labels (`1a`, `2a`, `3a`, `1b`, `2b`, `3b`) are kept
as aliases, but the primary state names below are descriptive and intended for
future code and discussions.

## Model

### External conditions

- `dc_power`: whether the board has DC supply power
- `video_source`: whether a computer is connected and actively presenting a
  usable video signal

### Inputs and events

- `dc_power_applied`
- `dc_power_removed`
- `power_button_pressed`
- `video_source_connected`
- `video_source_removed`
- timeouts completing during boot or no-signal handling

### Observables

- board LEDs: green, red, both off
- panel output: image visible, "no input", "entering power saving", panel off
- host behavior: whether the computer detects a display

### Design rule for firmware

Future code should track both:

- commanded intent, such as relay state or a requested button press
- observed board state, such as LEDs and display behavior

LEDs alone do not distinguish all relevant cases, especially during boot and
after DC power restoration.

## States

| State | Stable | Alias | Required conditions | Observables |
| --- | --- | --- | --- | --- |
| `DcOff` | yes | `1a`, `1b` | `dc_power = off` | LEDs off, panel off, host does not detect a display |
| `StandbyUnpoweredLogic` | yes | `2a`, `2b` | `dc_power = on`, controller soft-off | LEDs off, panel off, host does not detect a display |
| `BootingNoSignal` | no | none | `dc_power = on`, power-on path entered, no active video | Green LED on, panel turns on after about 15 seconds, then shows "no input" and "entering power saving" |
| `NoSignalSleep` | yes | `3a` | `dc_power = on`, controller soft-on but no active video | Red LED on, green LED off, panel off |
| `BootingWithVideo` | no | none | `dc_power = on`, power-on or wake path entered, active video present | Green LED on, host detects the display, panel shows image after about 5 to 12 seconds depending on entry path |
| `ActiveVideo` | yes | `3b` | `dc_power = on`, controller soft-on, active video present | Green LED on, red LED off, panel shows computer image, host detects the display |

## State Notes

### `DcOff`

- This is the hard-off state with no DC input to the board.
- The original notes distinguish `1a` and `1b` by cable presence, but the board
  itself behaves the same. The host-facing behavior differs only because the
  board is fully unpowered.

### `StandbyUnpoweredLogic`

- DC power is present, but the display controller is still soft-off.
- Pressing the board power button is required to enter an on state, unless the
  board is restoring a previously retained soft-on mode after a DC loss.

### `BootingNoSignal`

- This is the transient entered when the board is powered on without a video
  source.
- The observed timing was:
  - green LED turns on immediately
  - panel turns on after about 15 seconds
  - panel shows "no input", then "entering power saving"
  - about 10 seconds later, the panel turns off and the board settles in
    `NoSignalSleep`

### `NoSignalSleep`

- This is a stable soft-on state without video.
- The board is not equivalent to `StandbyUnpoweredLogic`; it remembers this
  mode across DC loss and tries to return to it automatically on the next power
  restore.
- Pressing the board power button from this state returns the board to
  `StandbyUnpoweredLogic`.

### `BootingWithVideo`

- This is the transient entered when the board is powered on or awakened while
  a valid video source is present.
- Two entry paths were observed:
  - from `StandbyUnpoweredLogic` by pressing the power button, reaching an
    image in about 5 seconds
  - from `NoSignalSleep` by connecting a computer, reaching an image in about
    12 seconds

### `ActiveVideo`

- This is the normal on state with a valid video signal.
- Pressing the board power button returns the board to
  `StandbyUnpoweredLogic`.

## Transition Table

| From | Event | Guard | To | Notes |
| --- | --- | --- | --- | --- |
| `DcOff` | `dc_power_applied` | last retained soft-on mode was off | `StandbyUnpoweredLogic` | Observed when DC had previously been removed from `StandbyUnpoweredLogic` |
| `DcOff` | `dc_power_applied` | last retained soft-on mode was on with no video | `BootingNoSignal` | Green LED turns on immediately after restore |
| `StandbyUnpoweredLogic` | `power_button_pressed` | `video_source = absent` | `BootingNoSignal` | Green LED turns on immediately |
| `BootingNoSignal` | boot and no-signal sequence completes | no video becomes available | `NoSignalSleep` | About 15 seconds to panel on, then about 10 seconds to settle |
| `NoSignalSleep` | `power_button_pressed` | none | `StandbyUnpoweredLogic` | Red LED turns off |
| `StandbyUnpoweredLogic` | `power_button_pressed` | `video_source = present` | `BootingWithVideo` | Host detects the display promptly |
| `BootingWithVideo` | image appears | entry came from `StandbyUnpoweredLogic` | `ActiveVideo` | Observed in about 5 seconds |
| `NoSignalSleep` | `video_source_connected` | valid signal becomes available | `BootingWithVideo` | Red LED turns off, green LED turns on |
| `BootingWithVideo` | image appears | entry came from `NoSignalSleep` | `ActiveVideo` | Observed in about 12 seconds |
| `ActiveVideo` | `power_button_pressed` | none | `StandbyUnpoweredLogic` | Green LED turns off, panel off |
| `StandbyUnpoweredLogic` | `dc_power_removed` | none | `DcOff` | Immediate hard-off |
| `NoSignalSleep` | `dc_power_removed` | none | `DcOff` | Red LED turns off in about 2 seconds |
| `ActiveVideo` | `dc_power_removed` | none | `DcOff` | Expected hard-off; not directly characterized in these experiments |

## Retained Behavior Across DC Loss

The board appears to retain whether it was last in a soft-off or soft-on mode
when DC power is removed.

Observed facts:

- If DC power is removed while the board is in `StandbyUnpoweredLogic`, then
  restoring DC returns the board to `StandbyUnpoweredLogic`.
- If DC power is removed while the board is in `NoSignalSleep`, then restoring
  DC causes the board to immediately resume the soft-on path:
  - green LED turns on
  - the panel powers up
  - no-input handling runs
  - the board returns to `NoSignalSleep`

Inference:

- `ActiveVideo` is also a soft-on mode, so future firmware should assume the
  board may attempt to auto-resume from it after DC restore.
- This was not directly measured here and must not be treated as confirmed
  behavior until characterized.

## Behavior by Experiment

### Experiment A: video cable always disconnected

- `DcOff` -> `StandbyUnpoweredLogic` on `dc_power_applied`
- `StandbyUnpoweredLogic` -> `BootingNoSignal` on `power_button_pressed`
- `BootingNoSignal` -> `NoSignalSleep` after the no-input sequence
- `NoSignalSleep` -> `StandbyUnpoweredLogic` on `power_button_pressed`
- `NoSignalSleep` -> `DcOff` on `dc_power_removed`
- `DcOff` -> `BootingNoSignal` on `dc_power_applied` if the retained mode was
  the prior soft-on no-video state

### Experiment B: video cable connected the entire time

- `DcOff` -> `StandbyUnpoweredLogic` on `dc_power_applied`
- `StandbyUnpoweredLogic` -> `BootingWithVideo` on `power_button_pressed`
- `BootingWithVideo` -> `ActiveVideo` when the image appears
- `ActiveVideo` -> `StandbyUnpoweredLogic` on `power_button_pressed`

### Experiment C: video cable connected halfway

- start in `NoSignalSleep`
- `NoSignalSleep` -> `BootingWithVideo` on `video_source_connected`
- `BootingWithVideo` -> `ActiveVideo` when the image appears
- `ActiveVideo` -> `StandbyUnpoweredLogic` on `power_button_pressed`

## Timing Notes

All timings here are observed and approximate. They should be used as planning
guidance, not as strict protocol requirements.

- `StandbyUnpoweredLogic` -> visible panel output without video: about 15
  seconds
- no-input screen to settled red-LED sleep: about 10 seconds
- `StandbyUnpoweredLogic` -> visible image with video already present: about 5
  seconds
- `NoSignalSleep` -> visible image after video is connected: about 12 seconds
- red LED extinguishes after DC removal from `NoSignalSleep`: about 2 seconds

## Implementation Guidance

This spec is intended to guide the future display power state machine.

Current building blocks already exist:

- relay control for board DC power in
  [`src/task/power_relay.rs`](/Users/abreis/Developer/Embedded/imac-5k-control/src/task/power_relay.rs)
- board LED observation and power-button actuation in
  [`src/task/pin_control.rs`](/Users/abreis/Developer/Embedded/imac-5k-control/src/task/pin_control.rs)

Recommended modeling rules for firmware:

- Treat relay state as control over `dc_power`, not as proof of the board's
  actual state.
- Treat LED observations as board-state evidence, but not the only source of
  truth.
- Represent transient states explicitly, because boot timing differs depending
  on whether video is already present.
- Preserve a distinction between:
  - hard-off due to DC removal
  - soft-off in `StandbyUnpoweredLogic`
  - soft-on with no video in `NoSignalSleep`
  - soft-on with video in `ActiveVideo`
- Mark any code path that assumes retained soft-on resume after DC restore as
  an inference until `ActiveVideo` restore behavior is measured directly.

## Open Gaps

These behaviors were not characterized by the original experiment and should be
measured before they become hard requirements:

- whether `ActiveVideo` auto-resumes after DC restore
- what happens if video is removed while in `ActiveVideo`
- what happens if video is removed during `BootingWithVideo`
- whether any LED combination other than off, green-only, and red-only ever
  appears during edge cases or faults
