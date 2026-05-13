#![no_std]
#![no_main]

use core::cell::RefCell;
use cortex_m::interrupt::{Mutex, free};
use cortex_m_rt::entry;
use nb::block;
use panic_rtt_target as _;
use ring_buffer::{Consumer, Producer, RingBuffer};
use rtt_target::{rprintln, rtt_init_print};
use stm32f1xx_hal::pac::USART1;
use stm32f1xx_hal::serial::Instance;
use stm32f1xx_hal::{
    pac::{self, interrupt},
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

// Ring buffer, start at 64 bytes considering 9600 baud from esp;
const USART1_RB_SIZE: usize = 64;

// Ring Buffer Producer
static PRODUCER: Mutex<RefCell<Option<Producer<'static, USART1_RB_SIZE>>>> =
    Mutex::new(RefCell::new(None));

static USART1_RX: Mutex<RefCell<Option<Rx<USART1>>>> = Mutex::new(RefCell::new(None));

enum EspResponse {
    Ok,
    Error,
    Fail,
    WifiGotIp,
    Ip([u8; 15], usize), // ip, len
    Other,
}

// TODO: need status for esp
struct Esp<USART> {
    tx: Tx<USART>,
    rb: Consumer<'static, USART1_RB_SIZE>,
}

impl<USART: Instance> Esp<USART> {
    fn new(tx: Tx<USART>, consumer: Consumer<'static, USART1_RB_SIZE>) -> Self {
        Self { tx, rb: consumer }
    }

    // TODO: error type
    fn init(&mut self) -> Result<(), ()> {
        self.send(b"AT\r\n");
        self.wait_for_ok();
        Ok(())
    }

    fn connect_wifi(&mut self, ssid: &str, pass: &str) -> Result<(), ()> {
        self.send(b"AT+CWJAP_CUR=\"");
        self.send(ssid.as_bytes());
        self.send(b"\",\"");
        self.send(pass.as_bytes());
        self.send(b"\"\r\n");
        rprintln!("Sent: CWJAP");

        if !matches!(self.wait_for_ok(), EspResponse::Ok) {
            return Err(());
        }

        let mut buf = [0u8; 128];
        self.send(b"AT+CIFSR\r\n");
        loop {
            let len = self.read_line(&mut buf);
            let line = &buf[..len];

            match Self::parse_line(line) {
                EspResponse::Ip(ip, ip_len) => {
                    rprintln!(
                        "WiFi Connected. IP {:?}",
                        core::str::from_utf8(&ip[0..ip_len]).unwrap_or("?")
                    );
                }
                EspResponse::Ok => break,
                _ => {}
            }
        }

        Ok(())
    }

    fn configure_server(&mut self, port: &[u8]) {
        self.send(b"AT+CIPMUX=1\r\n");
        self.wait_for_ok();

        self.send(b"AT+CIPSERVER=1,");
        self.send(&port);
        self.send(b"\r\n");
        self.wait_for_ok();
    }

    // Reads lines until OK, ERROR, or FAIL. Logs intermediate status responses.
    fn wait_for_ok(&mut self) -> EspResponse {
        let mut buf = [0u8; 128];
        loop {
            let len = self.read_line(&mut buf);
            let resp = Self::parse_line(&buf[..len]);
            match resp {
                EspResponse::Ok => {
                    rprintln!("OK");
                    return resp;
                }
                EspResponse::Error => {
                    rprintln!("ERROR");
                    return resp;
                }
                EspResponse::Fail => {
                    rprintln!("FAIL");
                    return resp;
                }
                EspResponse::WifiGotIp => {
                    rprintln!("WIFI GOT IP");
                }
                _ => {}
            }
        }
    }

    fn listen_loop(&mut self) {
        let mut body_buf = [0u8; 64];

        loop {
            self.wait_for(b"+IPD,");
            let len = self.handle_ipd(&mut body_buf);
            rprintln!(
                "body: {:?}",
                core::str::from_utf8(&body_buf[..len]).unwrap_or("?")
            );
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
                content_length = Self::parse_usize_from_slice(&line[16..]);
            }
        }

        let read_len = content_length.min(body_buf.len());
        for i in 0..read_len {
            body_buf[i] = self.read_byte();
        }

        read_len
    }

    fn wait_for(&mut self, pattern: &[u8]) {
        let len = pattern.len();
        let mut found = 0;

        // TODO: need timeout
        loop {
            if let Some(byte) = self.rb.read_byte() {
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
            match self.rb.read_byte() {
                Some(b) => {
                    buf[i] = b;
                    i += 1;
                    if i >= 2 && buf[i - 2] == b'\r' && buf[i - 1] == b'\n' {
                        break;
                    }
                }
                None => {}
            }
        }
        i
    }

    fn read_byte(&mut self) -> u8 {
        loop {
            match self.rb.read_byte() {
                Some(b) => return b,
                None => {}
            }
        }
    }

    // return type is (parsed_int, terminator byte [first non-integer])
    fn parse_u32(&mut self) -> Option<(u32, u8)> {
        let mut parsed = 0;
        let terminator;

        loop {
            if let Some(byte) = self.rb.read_byte() {
                if byte < b'0' || byte > b'9' {
                    terminator = byte;
                    break;
                }

                parsed = parsed * 10 + (byte - b'0') as u32
            }
        }

        Some((parsed, terminator))
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
}

#[entry]
fn main() -> ! {
    rtt_init_print!();
    rprintln!("STM32 ESP-12S starting...");
    rprintln!("SSID & PASS: {:?} {:?}", SSID, PASS);

    let dp = pac::Peripherals::take().unwrap();
    let mut flash = dp.FLASH.constrain();
    let mut afio = dp.AFIO.constrain();
    let rcc = dp.RCC.constrain();
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

    let (tx, mut rx) = serial.split();

    let (producer, consumer) = unsafe {
        static mut RB: RingBuffer<USART1_RB_SIZE> = RingBuffer::new();
        // `RB` is only accessible in this scope
        // and `main` is only called once.
        #[allow(static_mut_refs)]
        RB.split()
    };

    // enable RXNEIE, store rx and producer in statics, unmask USART1 in NVIC
    rx.listen();
    free(|cs| {
        PRODUCER.borrow(cs).replace(Some(producer));
        USART1_RX.borrow(cs).replace(Some(rx));
    });
    unsafe { pac::NVIC::unmask(pac::Interrupt::USART1) };

    rprintln!("Setting up serial");
    let mut esp = Esp::new(tx, consumer);

    rprintln!("Initializing ESP connection");
    esp.init().expect("Error initializing serial instance");
    rprintln!("Connecting Wifi");
    esp.connect_wifi(SSID, PASS).expect("Error connecting wifi");
    rprintln!("wifi connected");
    esp.configure_server(b"80");
    rprintln!("server configured");
    esp.listen_loop();

    loop {}
}

#[stm32f1xx_hal::pac::interrupt]
fn USART1() {
    free(|cs| {
        let mut rx = USART1_RX.borrow(cs).borrow_mut();
        let mut producer = PRODUCER.borrow(cs).borrow_mut();
        if let (Some(rx), Some(p)) = (rx.as_mut(), producer.as_mut()) {
            loop {
                if let Ok(b) = rx.read() {
                    if !p.write_byte(b) {
                        rprintln!("DEBUG: buffer full. Byte discarded");
                    }
                } else {
                    break;
                }
            }
        }
    })
}
