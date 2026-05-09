# stm32-esp12s

An interrupt-driven HTTP server on an STM32F103 ("Blue Pill"), using an OSOyoo WiFi Shield v1.3 (ESP-12S module) over UART.

The MCU connects to a home network via the ESP8266's AT command interface and listens for inbound HTTP POST requests. Received request bodies are logged over RTT.

## Architecture

UART receive is interrupt-driven. The USART1 ISR fires on each received byte, pushing it into a lock-free SPSC ring buffer ([ring-buffer](../ring-buffer)). The main loop pops from the consumer end, so no bytes are dropped due to polling latency.

```
ESP8266 → USART1 hardware → USART1 ISR → RingBuffer (producer)
                                                  ↓
                                    Esp struct (consumer) → AT response parser
```

The `Esp` struct drives the full lifecycle over AT commands: `AT` handshake → `AT+CWJAP_CUR` WiFi join → `AT+CIFSR` IP fetch → `AT+CIPMUX` + `AT+CIPSERVER` TCP server setup → `+IPD` receive loop.

## Hardware

**Board:** STM32F103C8T6 (Blue Pill)  
**Shield:** OSOyoo WiFi Shield v1.3 (ESP-12S)

The shield defaults to software UART on D4/D5. Set the jumpers to hardware UART mode (`E_TX↔TX`, `E_RX↔RX`), then run two wires to USART1 (which avoids the ST-Link virtual COM port conflict on USART2):

| Shield pin | Blue Pill pin | USART1 role |
|------------|---------------|-------------|
| E_RX       | D8 (PA9)      | TX          |
| E_TX       | D2 (PA10)     | RX          |

## Building and flashing

Requires a probe (ST-Link v2) and [`probe-rs`](https://probe.rs).

```sh
# build
cargo build --release

# flash + attach RTT console
cargo run --release
```

WiFi credentials are passed via environment variables at build time:

```sh
WIFI_SSID="your-network" WIFI_PASS="your-password" cargo run --release
```

## Dependencies

- [`stm32f1xx-hal`](https://github.com/stm32-rs/stm32f1xx-hal) — HAL for STM32F1
- [`ring-buffer`](https://github.com/peterwht/ring-buffer) — SPSC ring buffer used for interrupt-driven UART receive
- [`rtt-target`](https://github.com/mvirkkunen/rtt-target) — RTT logging
- [`cortex-m`](https://github.com/rust-embedded/cortex-m) — Cortex-M interrupt utilities
