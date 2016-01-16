#[macro_use]
extern crate log;
use log::{LogRecord, LogLevelFilter};

extern crate env_logger;
use env_logger::LogBuilder;

use std::env;

extern crate time;
extern crate rustc_serialize;

extern crate docopt;
use docopt::Docopt;

mod scale_server;
mod scale_listener;
use scale_server::ScaleServer;

#[cfg_attr(rustfmt, rustfmt_skip)]
const USAGE: &'static str = "
Scale Server: listens for incoming UDP scale packets and forwards them to websocket clients.

Usage:
  scale_server [-w <websocket-listen-addr>] [-s <scale-listen-addr>]
  scale_server -h

Options:
  -w ADDR, --websocket-listen-addr=ADDR  Listen for shit [default: 0.0.0.0:6002].
  -s ADDR, --scale-listen-addr=ADDR      Message [default: 0.0.0.0:6002].
  -h, --help                             Print this message.
";

#[derive(Debug, RustcDecodable)]
struct Args {
    flag_scale_listen_addr: String,
    flag_websocket_listen_addr: String,
}

fn main() {
    init_logging();
    let args: Args = Docopt::new(USAGE).and_then(|d| d.decode()).unwrap_or_else(|e| e.exit());

    match ScaleServer::start(&args.flag_scale_listen_addr,
                             &args.flag_websocket_listen_addr) {
        Err(err) => error!("Scale server exiting: {}", err),
        _ => (),
    }
}

fn init_logging() {
    let format = |record: &LogRecord| {
        let t = time::now();
        format!("{},{:03} - {} - {}",
                time::strftime("%Y-%m-%d %H:%M:%S", &t).unwrap(),
                t.tm_nsec / 1000_000,
                record.level(),
                record.args())
    };

    let mut builder = LogBuilder::new();
    builder.format(format).filter(None, LogLevelFilter::Info);

    if env::var("RUST_LOG").is_ok() {
        builder.parse(&env::var("RUST_LOG").unwrap());
    }

    builder.init().unwrap();
}
