use anyhow::{Context, Result};
use log::{error, info};
use niri_ipc::socket::Socket;
use niri_ipc::{Event, Request};
use std::env;

mod connection;
mod manager;

#[cfg(test)]
mod tests;

use crate::connection::SocketConnection;
use crate::manager::NiriContext;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = env::args().collect();
    let resize_columns = args.iter().any(|a| a == "--resize-columns");

    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage: niritiling [OPTIONS]");
        println!();
        println!("Options:");
        println!(
            "  --resize-columns  When a column is resized, adjust the other column to compensate"
        );
        println!("  -h, --help        Show this help message");
        return Ok(());
    }

    info!("niritiling: starting (resize_columns={})", resize_columns);

    loop {
        if let Err(e) = run_event_loop(resize_columns) {
            error!(
                "fatal error in event loop: {:?}. attempting to reconnect in 5 seconds...",
                e
            );
            std::thread::sleep(std::time::Duration::from_secs(5));
        } else {
            info!("event loop exited normally. restarting...");
        }
    }
}

fn run_event_loop(resize_columns: bool) -> Result<()> {
    let conn = SocketConnection::new()?;
    let mut context = NiriContext::new(Box::new(conn), resize_columns);

    let mut event_socket = Socket::connect().context("connecting to niri event stream")?;
    let _ = event_socket
        .send(Request::EventStream)
        .context("failed to request event stream")?;
    let mut read_event = event_socket.read_events();

    info!("connected to niri; performing initial synchronization");
    let state = context
        .connection
        .query_full_state()
        .context("initial state query failed")?;
    context.handle_event(Event::WindowsChanged {
        windows: state.windows,
    })?;

    loop {
        let event = match read_event().context("reading event from niri") {
            Ok(ev) => ev,
            Err(e) => {
                error!(
                    "error reading from event socket: {:?}. triggering reconnection...",
                    e
                );
                return Err(e);
            }
        };

        if let Err(e) = context.handle_event(event) {
            error!("error handling event: {:?}", e);
            if e.to_string().contains("connection") || e.to_string().contains("socket") {
                return Err(e);
            }
        }
    }
}
