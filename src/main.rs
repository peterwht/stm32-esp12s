#![no_std]
#![no_main]

use cortex_m_rt::entry;
use nb::block;
use panic_halt as _;
use rtt_target::{rtt_init_print, rprintln};
use stm32f1xx_hal::{
    pac::{self, USART1},
    prelude::*,
    serial::{Config, Rx, Serial, Tx},
};

// OSOyoo WiFi Shield v1.3 — ESP-12S AT command interface
//
// Shield jumper options:
//   Software UART: D4↔E_TX / D5↔E_RX — default, bit-banged, not supported by hal
//   Hardware UART: E_TX↔TX / E_RX↔RX — routes to D0/D1 (PA2/PA3, USART2)
//
// USART2 (PA2/PA3) conflicts with ST-Link virtual COM port, so we use USART1 instead.
// With hardware UART jumpers set, run manual wires:
//   Shield E_RX → D8 (PA9, USART1 TX)
//   Shield E_TX → D2 (PA10, USART1 RX)

const SSID: &str = env!("WIFI_SSID");
const PASS: &str = env!("WIFI_PASS");

enum EspResponse {
    Ok,
    Error,
    Fail,
    WifiGotIp,
    Other,
}

fn send(tx: &mut Tx<USART1>, data: &[u8]) {
    for byte in data {
        block!(tx.write(*byte)).ok();
    }
}

fn read_line(rx: &mut Rx<USART1>, buf: &mut [u8]) -> usize {
    let mut i = 0;
    while i < buf.len() {
        let b = block!(rx.read()).unwrap_or(0);
        buf[i] = b;
        i += 1;
        if i >= 2 && buf[i - 2] == b'\r' && buf[i - 1] == b'\n' {
            break;
        }
    }
    i
}

fn parse_line(line: &[u8]) -> EspResponse {
    match line {
        b"OK\r\n" => EspResponse::Ok,
        b"ERROR\r\n" => EspResponse::Error,
        b"FAIL\r\n" => EspResponse::Fail,
        b"WIFI GOT IP\r\n" => EspResponse::WifiGotIp,
        _ => EspResponse::Other,
    }
}

#[entry]
fn main() -> ! {
    rtt_init_print!();
    rprintln!("STM32 ESP-12S starting...");

    let dp = pac::Peripherals::take().unwrap();

    let mut flash = dp.FLASH.constrain();
    let rcc = dp.RCC.constrain();
    let mut afio = dp.AFIO.constrain();
    let clocks = rcc.cfgr.freeze(&mut flash.acr);

    let mut gpioa = dp.GPIOA.split();

    // USART1: PA9 (TX), PA10 (RX) — no ST-Link conflict
    let tx = gpioa.pa9.into_alternate_push_pull(&mut gpioa.crh);
    let rx = gpioa.pa10;

    let serial = Serial::new(
        dp.USART1,
        (tx, rx),
        &mut afio.mapr,
        Config::default().baudrate(9_600.bps()),
        &clocks,
    );

    let (mut tx, mut rx) = serial.split();

    let mut buf = [0u8; 128];

    send(&mut tx, b"AT+CWJAP_CUR=\"");
    send(&mut tx, SSID.as_bytes());
    send(&mut tx, b"\",\"");
    send(&mut tx, PASS.as_bytes());
    send(&mut tx, b"\"\r\n");
    rprintln!("Sent: CWJAP");

    let mut pending: Option<&[u8]> = None;

    loop {
        let len = read_line(&mut rx, &mut buf);
        let line = &buf[..len];

        rprintln!("line: {:?}", core::str::from_utf8(line).unwrap_or("?"));

        match parse_line(line) {
            EspResponse::Ok => {
                rprintln!("OK");
                pending = Some(b"AT+CIFSR\r\n");
            },
            EspResponse::Error => rprintln!("ERROR"),
            EspResponse::Fail => rprintln!("FAIL"),
            EspResponse::WifiGotIp => {
                rprintln!("WIFI GOT IP");
            },
            EspResponse::Other => {}
        }

        if let Some(cmd) = pending {
            rprintln!("cmd: {:?}", core::str::from_utf8(cmd).unwrap());
            send(&mut tx, cmd);
            pending = None;
        }
    }
}
