#![no_std]
#![no_main]

use cortex_m::asm;
use cortex_m_rt::entry;
use nb::block;
use panic_rtt_target as _;
use rtt_target::{rprintln, rtt_init_print};
use stm32f1xx_hal::afio::MAPR;
use stm32f1xx_hal::rcc::Clocks;
use stm32f1xx_hal::serial::{Instance, Pins};
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
//
// USART1 supports interrupt on read by setting the RXNEIE bit in the USART_CR1 register
//

const SSID: &str = env!("WIFI_SSID");
const PASS: &str = env!("WIFI_PASS");

enum EspResponse {
    Ok,
    Error,
    Fail,
    WifiGotIp,
    Ip([u8; 15], usize), // ip, len
    Other,
}

fn parse_usize_from_slice(data: &[u8]) -> usize {
    let mut n: usize = 0;
    for &b in data {
        if b < b'0' || b > b'9' {
            break;
        }
        n = n * 10 + (b - b'0') as usize;
    }
    n
}

fn parse_line(line: &[u8]) -> EspResponse {
    match line {
        b"OK\r\n" => EspResponse::Ok,
        b"ERROR\r\n" => EspResponse::Error,
        b"FAIL\r\n" => EspResponse::Fail,
        b"WIFI GOT IP\r\n" => EspResponse::WifiGotIp, // TODO: probably don't need
        l if l.starts_with(b"+CIFSR:STAIP,\"") => {
            let mut ip = [0u8; 15];
            let ip_len = l.len() - 3 - 14;
            ip[..ip_len].copy_from_slice(&l[14..l.len() - 3]);
            EspResponse::Ip(ip, ip_len)
        }
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
        Self { tx, rx }
    }

    // TODO: error type
    fn init(&mut self) -> Result<(), ()> {
        self.send(b"AT\r\n");
        let mut buf = [0u8; 128];

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
                }
                EspResponse::Error => {
                    rprintln!("ERROR");
                    break;
                }
                EspResponse::Fail => {
                    rprintln!("FAIL");
                    break;
                }
                EspResponse::WifiGotIp => {
                    rprintln!("WIFI GOT IP");
                }
                EspResponse::Other => {}
                _ => {}
            }
        }

        Ok(())
    }

    fn connect_wifi(&mut self, ssid: &str, pass: &str) -> Result<(), ()> {
        self.send(b"AT+CWJAP_CUR=\"");
        self.send(ssid.as_bytes());
        self.send(b"\",\"");
        self.send(pass.as_bytes());
        self.send(b"\"\r\n");
        rprintln!("Sent: CWJAP");

        // TODO: dry

        let mut buf = [0u8; 128];

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
                }
                EspResponse::Error => {
                    rprintln!("ERROR");
                    break;
                }
                EspResponse::Fail => {
                    rprintln!("FAIL");
                    break;
                }
                EspResponse::WifiGotIp => {
                    rprintln!("WIFI GOT IP");
                }
                EspResponse::Other => {}
                _ => {}
            }
        }

        if !matches!(resp, EspResponse::Ok) {
            return Err(());
        }

        self.send(b"AT+CIFSR\r\n");
        loop {
            let len = self.read_line(&mut buf);
            let line = &buf[..len];
            rprintln!("line: {:?}", core::str::from_utf8(line).unwrap_or("?"));

            match parse_line(line) {
                EspResponse::Ip(ip, ip_len) => {
                    rprintln!(
                        "WiFi Connected. IP {:?}",
                        core::str::from_utf8(&ip[0..ip_len]).unwrap_or("?")
                    );
                }
                EspResponse::Ok => {
                    break;
                }
                _ => {} // TODO: not handling errors
            }
        }

        Ok(())
    }

    fn configure_server(&mut self, port: &[u8]) {
        self.send(b"AT+CIPMUX=1\r\n");

        let mut buf = [0u8; 128];
        let mut resp: EspResponse;

        loop {
            let len = self.read_line(&mut buf);
            let line = &buf[..len];
            rprintln!("line: {:?}", core::str::from_utf8(line).unwrap_or("?"));

            resp = parse_line(line);
            if let EspResponse::Ok = resp {
                break;
            }
        }

        self.send(b"AT+CIPSERVER=1,");
        self.send(&port);
        self.send(b"\r\n");

        loop {
            let len = self.read_line(&mut buf);
            let line = &buf[..len];
            rprintln!("line: {:?}", core::str::from_utf8(line).unwrap_or("?"));

            resp = parse_line(line);
            if let EspResponse::Ok = resp {
                break;
            }
        }
    }

    fn find(&mut self, pattern: &[u8]) -> bool {
        let len = pattern.len();
        let mut found = 0;

        // TODO: need timeout
        loop {
            let byte = block!(self.rx.read()).unwrap(); //TODO: unwrap
            if byte == pattern[found as usize] {
                found += 1;
            } else {
                found = 0;
                // recheck if current byte matches pattern start
                if byte == pattern[found as usize] {
                    found = 1;
                }
            }

            if found == len {
                break;
            }
        }

        true
    }

    fn read_byte(&mut self) -> u8 {
        loop {
            match block!(self.rx.read()) {
                Ok(b) => return b,
                Err(_) => {}
            }
        }
    }

    fn handle_ipd(&mut self, body_buf: &mut [u8]) -> usize {
        self.read_byte(); // conn id digit
        self.read_byte(); // ','
        self.parse_u32(); // IPD length, terminator ':' consumed

        let mut content_length: usize = 0;
        let mut line_buf = [0u8; 128];

        loop {
            let len = self.read_line(&mut line_buf);
            let line = &line_buf[..len];

            if line == b"\r\n" {
                break;
            }

            if line.starts_with(b"Content-Length: ") {
                content_length = parse_usize_from_slice(&line[16..]);
            }
        }

        let read_len = content_length.min(body_buf.len());
        for i in 0..read_len {
            body_buf[i] = self.read_byte();
        }

        read_len
    }

    fn listen_loop(&mut self) {
        let mut body_buf = [0u8; 64];

        loop {
            if self.find(b"+IPD,") {
                let len = self.handle_ipd(&mut body_buf);
                rprintln!(
                    "body: {:?}",
                    core::str::from_utf8(&body_buf[..len]).unwrap_or("?")
                );
            }
        }
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

    // return type is (parsed_int, terminator byte [first non-integer])
    fn parse_u32(&mut self) -> Option<(u32, u8)> {
        let mut parsed = 0;
        let terminator;

        loop {
            let byte = block!(self.rx.read()).unwrap_or(b'?'); // if read failed, no more integer to parse

            if byte < b'0' || byte > b'9' {
                terminator = byte;
                break;
            }

            parsed = parsed * 10 + (byte - b'0') as u32
        }

        Some((parsed, terminator))
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
    esp.configure_server(b"80");

    esp.listen_loop();

    loop {}
}
