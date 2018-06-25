#![allow(non_camel_case_types)]

use grains::*;
use redbpf::{LoadError, PerfMap, Result};
use serde::{Deserialize, Serialize};

use std::env;
use std::net::Ipv4Addr;
use std::ptr;
use std::thread;
use std::time::Duration;

use cadence::StatsdClient;

pub struct TCP4;

const MODULE_TCPV4: &'static [u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tcpv4.elf"));
include!(concat!(env!("OUT_DIR"), "/tcpv4.rs"));

impl<'m> EBPFModule<'static, 'm> for TCP4 {
    fn code() -> &'static [u8] {
        MODULE_TCPV4
    }

    fn handler(m: &'m mut Map, statsd: &StatsdClient) -> Result<PerfMap<'m>> {
        match m.name.as_str() {
            "tcp4_connections" => PerfMap::new(m, -1, 0, 16, || {
                let statsd = statsd.clone();
                Box::new(move |raw| {
                    use cadence::prelude::*;

                    let connection = Connection::from(_data_connect::from(raw));
                    let sent = statsd
                        .incr_with_tags("connection.{}")
                        .with_tag("host", &format!("{}", connection.destination_ip))
                        .with_tag("port", &format!("{}", connection.destination_port))
                        .with_tag("name", &format!("{}", connection.name))
                        .try_send()
                        .unwrap();
                })
            }),
            "tcp4_volume" => PerfMap::new(m, -1, 0, 128, || {
                let statsd = statsd.clone();
                Box::new(move |raw| {
                    use cadence::prelude::*;
                    let volume = Volume::from(_data_volume::from(raw));

                    let vol = if volume.send > 0 {
                        volume.send
                    } else {
                        volume.recv
                    };
                    let _stat = statsd
                        .count_with_tags(
                            &format!("volume.{}", if volume.send > 0 { "out" } else { "in" }),
                            vol as i64,
                        )
                        .with_tag("host", &format!("{}", volume.connection.destination_ip))
                        .with_tag("port", &format!("{}", volume.connection.destination_port))
                        .with_tag("name", &format!("{}", volume.connection.name))
                        .with_tag("proto", "tcpv4")
                        .try_send()
                        .unwrap();
                })
            }),
            _ => Err(LoadError::BPF),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Volume {
    connection: Connection,
    send: usize,
    recv: usize,
}

impl From<_data_volume> for Volume {
    fn from(data: _data_volume) -> Volume {
        Volume {
            connection: Connection::from(data.conn),
            send: data.send,
            recv: data.recv,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Connection {
    pid: u32,
    name: String,
    source_ip: Ipv4Addr,
    destination_ip: Ipv4Addr,
    destination_port: u16,
    source_port: u16,
}

impl From<_data_connect> for Connection {
    fn from(data: _data_connect) -> Connection {
        Connection {
            pid: data.id as u32,
            name: get_string(unsafe { &*(&data.comm as *const [i8] as *const [u8]) }),
            source_ip: to_ip(data.saddr),
            destination_ip: to_ip(data.daddr),
            destination_port: (data.dport >> 8) | (data.dport << 8),
            source_port: (data.sport >> 8) | (data.sport << 8),
        }
    }
}

fn to_ip(bytes: u32) -> Ipv4Addr {
    let d = (bytes >> 24) as u8;
    let c = (bytes >> 16) as u8;
    let b = (bytes >> 8) as u8;
    let a = bytes as u8;

    Ipv4Addr::new(a, b, c, d)
}

fn get_string(x: &[u8]) -> String {
    match x.iter().position(|&r| r == 0) {
        Some(zero_pos) => String::from_utf8_lossy(&x[0..zero_pos]).to_string(),
        None => String::from_utf8_lossy(x).to_string(),
    }
}