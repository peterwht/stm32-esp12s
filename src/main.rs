#![no_std]
#![no_main]

use cortex_m_rt::entry;
use nb::block;
use panic_rtt_target as _;
use rtt_target::{rtt_init_print, rprintln};
use stm32f1xx_hal::{
    pac::{self, USART1},
    prelude::*,
    serial::{Config, Rx, Serial, Tx},
};
use stm32f1xx_hal::afio::MAPR;
use stm32f1xx_hal::rcc::Clocks;
use stm32f1xx_hal::serial::{Instance, Pins};
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
    Ip([u8;15], usize), // ip, len
    Other,
}



fn parse_line(line: &[u8]) -> EspResponse {
    match line {
        b"OK\r\n" => EspResponse::Ok,
        b"ERROR\r\n" => EspResponse::Error,
        b"FAIL\r\n" => EspResponse::Fail,
        b"WIFI GOT IP\r\n" => EspResponse::WifiGotIp,
        l if l.starts_with(b"+CIFSR:STAIP,\"") => {
            let mut ip = [0u8;15];
            let ip_len = l.len() - 3-14;
            ip[..ip_len].copy_from_slice(&l[14..l.len() - 3]);
            EspResponse::Ip(ip, ip_len)
        },
        _ => EspResponse::Other,
    }
}

// TODO: need status for esp
struct Esp<USART> {
    tx: Tx<USART>,
    rx: Rx<USART>,
}

impl<USART: Instance> Esp<USART> {
    fn new<PINS: Pins<USART>>(usart: USART, pins: PINS, mapr: &mut MAPR, clocks: &Clocks) -> Self {
        let serial = Serial::new(
            usart,
            pins,
            mapr,
            Config::default().baudrate(9_600.bps()),
            &clocks,
        );

        let (tx, rx) = serial.split();
        Self {
            tx,
            rx,
        }
    }

    // TODO: error type
    fn init(&mut self) -> Result<(), ()> {
        self.send(b"AT\r\n");
        let mut buf = [0u8;128];

        let mut resp: EspResponse;

        // TODO: improve, timeout, error handling, refactor parsing, etc.
        loop {
            let len = self.read_line(&mut buf);
            rprintln!("len: {}", len);
            let line = &buf[..len];
            rprintln!("line: {:?}", core::str::from_utf8(line).unwrap_or("?"));

            resp = parse_line(line);
            match resp {
                EspResponse::Ok => {
                    rprintln!("OK");
                    break;
                },
                EspResponse::Error => {
                    rprintln!("ERROR");
                    break;
                },
                EspResponse::Fail => {
                    rprintln!("FAIL");
                    break;
                },
                EspResponse::WifiGotIp => {
                    rprintln!("WIFI GOT IP");
                },
                EspResponse::Other => {}
                _ => {},
            }
        }

        Ok(())
    }

    fn connect_wifi(&mut self, ssid: &str, pass: &str) -> Result<(),()> {
        self.send(b"AT+CWJAP_CUR=\"");
        self.send(SSID.as_bytes());
        self.send(b"\",\"");
        self.send(PASS.as_bytes());
        self.send(b"\"\r\n");
        rprintln!("Sent: CWJAP");

        // TODO: dry

        let mut buf = [0u8;128];

        let mut resp: EspResponse;

        // TODO: improve, timeout, error handling, refactor parsing, etc.
        loop {
            let len = self.read_line(&mut buf);
            let line = &buf[..len];
            rprintln!("line: {:?}", core::str::from_utf8(line).unwrap_or("?"));

            resp = parse_line(line);
            match resp {
                EspResponse::Ok => {
                    rprintln!("OK");
                    break;
                },
                EspResponse::Error => {
                    rprintln!("ERROR");
                    break;
                },
                EspResponse::Fail => {
                    rprintln!("FAIL");
                    break;
                },
                EspResponse::WifiGotIp => {
                    rprintln!("WIFI GOT IP");
                },
                EspResponse::Other => {},
                _ => {}
            }
        }

        if !matches!(resp, EspResponse::Ok) {
            return Err(())
        }

        self.send(b"AT+CIFSR\r\n");
        loop {
            let len = self.read_line(&mut buf);
            let line = &buf[..len];
            rprintln!("line: {:?}", core::str::from_utf8(line).unwrap_or("?"));

            match parse_line(line) {
                EspResponse::Ip(ip, ip_len) => {
                    rprintln!("WiFi Connected. IP {:?}", core::str::from_utf8(&ip[0..ip_len]).unwrap_or("?"));
                },
                EspResponse::Ok => {
                    break;
                },
                _ => {} // TODO: not handling errors
            }
        }

        Ok(())
    }

    fn send(&mut self, data: &[u8]) {
        for byte in data {
            block!(self.tx.write(*byte)).ok();
        }
    }

    fn read_line(&mut self, buf: &mut [u8]) -> usize {
        let mut i = 0;
        while i < buf.len() {
            match block!(self.rx.read()) {
                Ok(b) => {
                    buf[i] = b;
                    i += 1;
                    if i >= 2 && buf[i - 2] == b'\r' && buf[i - 1] == b'\n' {
                        break;
                    }
                }
                Err(_) => {}
            }
        }
        i
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

    rprintln!("Setting up serial");
    let mut esp = Esp::new(dp.USART1, (tx, rx), &mut afio.mapr, &clocks);

    rprintln!("Initializing ESP connection");
    esp.init().expect("Error initializing serial instance");
    rprintln!("Connecting Wifi");
    esp.connect_wifi(SSID, PASS).expect("Error connecting wifi");

    loop {}
}
