use clap::{command, Parser, ValueEnum};
use reqwest::blocking::get;
use server::Server;
use util::{measure_latency, run_this_exe_as_server};

mod httpsys;
mod server;
mod util;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(value_enum, default_value_t = Mode::Help)]
    mode: Mode,

    #[arg(short = 'p', long = "port", default_value_t = 8080)]
    port_num: u16,

    #[arg(short = 'i', long = "ip", default_value = "localhost")]
    ip_address: String,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
enum Mode {
    Server,
    Client,
    Proxy,
    Test,
    Help,
}

fn main() {
    let args = Args::parse();

    match args.mode {
        Mode::Server => {
            let mut server = Server::new().unwrap();
            let nop_url = format!("http://localhost:{}/nop/", args.port_num);
            let kill_url = format!("http://localhost:{}/kill/", args.port_num);
            let handlers: Vec<(&str, fn(&str) -> (String, bool))> = vec![
                (&nop_url, |_| ("[\"OK\"]".to_string(), false)),
                (&kill_url, |_| ("[\"OK\"]".to_string(), true)),
            ];
            server.define_handlers(handlers);
            server.wait();
        }
        Mode::Client => {
            println!("Starting in client mode");
            let dest_url = format!("http://{}:{}/nop/", args.ip_address, args.port_num);

            let average_latency = measure_latency(|| {
                let _ = make_request(&dest_url);
            });

            println!("Average latency: {:?}", average_latency);
        }
        Mode::Proxy => {
            println!("Proxy mode");
            // ... Implement proxy logic
        }
        Mode::Test => {
            println!("Test mode");
            let server_exe = run_this_exe_as_server();

            println!("Server process started");
            println!("Calling server multiple times to measure latency");

            let url = server_exe.format_req_url("/nop/");

            let average_latency = measure_latency(|| {
                let _ = make_request(&url);
            });

            println!("Average latency: {:?}", average_latency);
        }
        Mode::Help => {
            println!("Network latency tester");
            println!("Usage: net-bench.exe <MODE> [OPTIONS]");
            println!("");
            println!("Modes:");
            println!("  server:   Starts the HTTP server.");
            println!("  client:   Sends requests to the server and measures latency.");
            println!("  proxy:    Acts as a proxy server.");
            println!("  test:     Starts this app as a server and measures latency.");
            println!("  help:     Displays this help message.");
            println!("");
            println!("Options:");
            println!("  -p, --port <PORT>     Sets the port number for the server (default: 8080)");
            println!("  -i, --ip <IP>         Sets the IP address of the server (default: localhost)");
        }
    }
}

pub fn make_request(url: &str) -> String {
    let response = get(url).expect("Failed to send request");
    if !response.status().is_success() {
        panic!("Failed request with status code: {}", response.status());
    }
    response.text().expect("Failed to read response body")
}

#[cfg(test)]
mod tests {
    use std::{thread, time::Duration};

    use super::*;
    use util::ServerExe;

    #[test]
    fn test_basic_request() {
        // let port_num = 1919;
        // let server = server::start_handling_requests(port_num).unwrap();

        // thread::sleep(Duration::from_secs(1));

        // let handle = ServerExe {
        //     proc: None,
        //     port: port_num as i32,
        // };

        // // Expect default request returns 'ok'
        // assert_eq!(handle.request("/nop/"), "ok");
        // server.wait();
    }
}
