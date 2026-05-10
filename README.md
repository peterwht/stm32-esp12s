# stm32-esp12s

An interrupt-driven HTTP server on an STM32F103 (NUCLEO-F103RB), using an OSOyoo WiFi Shield v1.3 (ESP-12S module) over UART.

The MCU connects to a home network via the ESP8266's AT command interface and listens for inbound HTTP POST requests. Received request bodies are logged over RTT.

## Architecture

UART receive is interrupt-driven. The USART1 ISR fires on each received byte, pushing it into a lock-free SPSC ring buffer ([ring-buffer](../ring-buffer)). The main loop pops from the consumer end, so no bytes are dropped due to polling latency.

```
ESP8266 → USART1 hardware → USART1 ISR → RingBuffer (producer)
                                                  ↓
                                    Esp struct (consumer) → AT response parser
```

The `Esp` struct drives the full lifecycle over AT commands: `AT` handshake → `AT+CWJAP_CUR` WiFi join → `AT+CIFSR` IP fetch → `AT+CIPMUX` + `AT+CIPSERVER` TCP server setup → `+IPD` receive loop.

### Concurrency

The ring buffer's `split()` enforces SPSC at compile time — the borrow checker prevents constructing a second `Producer` or `Consumer`, with no runtime cost.

The producer is shared between `main` (initialization) and the ISR (runtime) via `Mutex<RefCell<Option<Producer>>>`. `Mutex` from `cortex-m` restricts access to critical sections so the ISR cannot preempt `main` mid-access. `RefCell` adds a runtime check ensuring only one mutable borrow of the static is active at a time.

## Hardware

**Board:** NUCLEO-F103RB (Nucleo-64)  
**Shield:** OSOyoo WiFi Shield v1.3 (ESP-12S)

The shield defaults to software UART on D4/D5. Set the jumpers to hardware UART mode (`E_TX↔TX`, `E_RX↔RX`), then run two wires to USART1 (which avoids the ST-Link virtual COM port conflict on USART2):

| Shield pin | Nucleo pin    | USART1 role |
|------------|---------------|-------------|
| E_RX       | D8 (PA9)      | TX          |
| E_TX       | D2 (PA10)     | RX          |

## Building and flashing

Requires an ST-Link v2 and [`cargo-embed`](https://probe.rs/docs/tools/cargo-embed/).

```sh
cargo install cargo-embed
```

`Embed.toml` configures the target chip and enables RTT. Flash and attach the RTT console in one command:

```sh
WIFI_SSID="your-network" WIFI_PASS="your-password" cargo embed --release
```

## Dependencies

- [`stm32f1xx-hal`](https://github.com/stm32-rs/stm32f1xx-hal) — HAL for STM32F1
- [`ring-buffer`](https://github.com/peterwht/ring-buffer) — SPSC ring buffer used for interrupt-driven UART receive
- [`rtt-target`](https://github.com/mvirkkunen/rtt-target) — RTT logging
- [`cortex-m`](https://github.com/rust-embedded/cortex-m) — Cortex-M interrupt utilities
