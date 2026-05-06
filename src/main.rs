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
            let len = self.read_line(&mut buf, false);
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
        self.send(SSID.as_bytes());
        self.send(b"\",\"");
        self.send(PASS.as_bytes());
        self.send(b"\"\r\n");
        rprintln!("Sent: CWJAP");

        // TODO: dry

        let mut buf = [0u8; 128];

        let mut resp: EspResponse;

        // TODO: improve, timeout, error handling, refactor parsing, etc.
        loop {
            let len = self.read_line(&mut buf, false);
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
            let len = self.read_line(&mut buf, false);
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
            let len = self.read_line(&mut buf, false);
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
            let len = self.read_line(&mut buf, false );
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
            // rprintln!("matching {:?}", core::str::from_utf8(&byte.to_be_bytes()).unwrap_or("?"));
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



    fn listen_loop(&mut self) {
        loop {
            if self.rx.is_rx_not_empty() {
                if self.find(b"+IPD,") {
                    rprintln!("FOUND IPD");
                };

                let _ = block!(self.rx.read()); // <ID>
                let _ = block!(self.rx.read()); // ','

                let (bytes_received, _) = self.parse_u32().unwrap_or((0, b'0'));
                rprintln!("bytes_received: {}", bytes_received);
                if self.find(b"Content-Length: ") {
                    rprintln!("Content length found");
                }

                let (content_length, _) = self.parse_u32().unwrap_or((0, b'0'));
                rprintln!("content_length: {}", content_length);

                // if self.find(b"data=") {
                //     rprintln!("data found");
                // }

                for _ in 0..100 {
                    rprintln!("{:?}", core::str::from_utf8(&block!(self.rx.read()).expect("not empty").to_be_bytes()).unwrap_or("?"));
                }
                // rprintln!("{:?}", core::str::from_utf8(&block!(self.rx.read()).expect("not empty").to_be_bytes()).unwrap_or("?"));
                // rprintln!("{:?}", core::str::from_utf8(&block!(self.rx.read()).expect("not empty").to_be_bytes()).unwrap_or("?"));
                // rprintln!("{:?}", core::str::from_utf8(&block!(self.rx.read()).expect("not empty").to_be_bytes()).unwrap_or("?"));
                // rprintln!("{:?}", core::str::from_utf8(&block!(self.rx.read()).expect("not empty").to_be_bytes()).unwrap_or("?"));
                // rprintln!("{:?}", core::str::from_utf8(&block!(self.rx.read()).expect("not empty").to_be_bytes()).unwrap_or("?"));
                // rprintln!("{:?}", core::str::from_utf8(&block!(self.rx.read()).expect("not empty").to_be_bytes()).unwrap_or("?"));
                // rprintln!("{:?}", core::str::from_utf8(&block!(self.rx.read()).expect("not empty").to_be_bytes()).unwrap_or("?"));
                // rprintln!("{:?}", core::str::from_utf8(&block!(self.rx.read()).expect("not empty").to_be_bytes()).unwrap_or("?"));
                // let len = self.read_line(&mut buf, true);
                // let line = &buf[..len];
                // rprintln!("line: {:?}", core::str::from_utf8(line).unwrap_or("?"));
                break;
            }
            // for _ in 0..5_000 {
            //     asm::nop();
            // }
        }
    }

    fn send(&mut self, data: &[u8]) {
        for byte in data {
            block!(self.tx.write(*byte)).ok();
        }
    }

    fn read_line(&mut self, buf: &mut [u8], debug: bool) -> usize {
        let mut i = 0;
        while i < buf.len() {
            match block!(self.rx.read()) {
                Ok(b) => {
                    if debug {
                        // rprintln!("B: {:?}", core::str::from_utf8(&b.to_be_bytes()).unwrap_or("?"));
                    }
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
