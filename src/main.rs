use clap::{command, Parser, Subcommand};
use plotters::prelude::*;
use plotters::style::{BLUE, WHITE};
use rand::distributions::Alphanumeric;
use rand::prelude::Distribution;
use rand::thread_rng;
use reqwest::blocking::Client;
use reqwest::{Proxy, Url};
use server::Server;
use std::collections::BTreeMap;
use std::error::Error;
use std::time::Instant;
use std::{thread, time::Duration};
use tokio::signal;
use tokio::task;
use util::print_latency;
use util::{measure_latency, run_this_exe_as_server};

mod httpsys;
mod server;
mod util;

/// Network latency tester.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Mode,

    /// Validate SSL certificates. If set to false, invalid certificates will be accepted.
    /// This is useful for development or testing with self-signed certificates but is
    /// not recommended for production environments due to security risks.
    #[arg(
        short,
        long,
        default_value_t = false,
        help = "Don't Validate SSL certificates"
    )]
    no_validate_certs: bool,
}

#[derive(Subcommand, Debug, Clone)]
enum Mode {
    /// Starts the HTTP server.
    #[command(alias = "s")]
    Server {
        #[arg(help = "The URL to receive requests on", default_value = "http://localhost:8080", value_parser = is_valid_url)]
        receive_url: Url,
    },
    /// Sends requests to the server and measures latency.
    #[command(alias = "c")]
    Client {
        #[arg(help = "The URL to send requests to", default_value = "http://localhost:8080", value_parser = is_valid_url)]
        send_url: Url,
        #[arg(help = "Optional proxy server URL (example http://localhost:8080)")]
        proxy_url: Option<Url>,
    },
    /// Sends requests to the server and prints the result.
    #[command(alias = "e")]
    Echo {
        #[arg(help = "The URL to send requests to", default_value = "http://localhost:8080", value_parser = is_valid_url)]
        send_url: Url,
        #[arg(help = "Optional proxy server URL (example http://localhost:8080)")]
        proxy_url: Option<Url>,
    },
    /// Starts this app as a server and measures latency.
    #[command(alias = "t")]
    Test,
}

fn is_valid_url(url: &str) -> Result<Url, String> {
    Url::parse(url).map_err(|error| error.to_string())
}

fn main() {
    let args = Args::parse();

    match &args.command {
        Mode::Server { receive_url } => {
            println!("Server running on {receive_url}/test/");
            let mut server = Server::new();
            let test_url = {
                let mut url = receive_url.clone();
                url.set_path("/test");
                url
            };
            let kill_url = {
                let mut url = receive_url.clone();
                url.set_path("/kill");
                url
            };
            let handlers: Vec<(&Url, fn(&str) -> (String, bool))> = vec![
                (&test_url, |_| ("OK".to_string(), false)),
                (&kill_url, |_| ("OK".to_string(), true)),
            ];
            server.define_handlers(handlers);
            server.wait();
        }
        Mode::Client {
            send_url,
            proxy_url,
        } => {
            println!("Client sending to: {send_url}");
            println!("Validate SSL certificates: {}", !args.no_validate_certs);

            let average_latency = measure_latency(|| {
                let _ = send_get_request(send_url, proxy_url, args.no_validate_certs);
            });

            print_latency(&average_latency);
        }
        Mode::Echo {
            send_url,
            proxy_url,
        } => {
            println!("Client sending to: {send_url}");
            println!("Validate SSL certificates: {}", !args.no_validate_certs);

            let start_time = Instant::now();
            let result = send_get_request(send_url, proxy_url, args.no_validate_certs);
            let latency = start_time.elapsed();
            let mut response_size = 0;

            println!("============================================================");

            match result {
                Ok(value) => {
                    println!("{}", value);
                    response_size = value.len();
                }
                Err(e) => eprintln!("Error: {}", e),
            };

            println!("============================================================");
            println!("Latency: {:?}", latency);
            println!("Response Size: {} chars", response_size);
        }
        Mode::Test => {
            println!("Test mode");
            let server_exe = run_this_exe_as_server();

            println!("Server process started");
            println!("Calling server multiple times to measure latency");

            thread::sleep(Duration::from_millis(100));

            let send_url = server_exe.format_req_url("/test/");
            let mut measurements = Vec::<Measurement>::new();
            let mut payload_size = 1024; // Initial payload size
            let target_size = 8 * 1024 * 1024; // 8 MB

            while payload_size <= target_size {
                let random_data = generate_random_payload(payload_size);
                let latency_result = measure_latency(|| {
                    task::block_in_place(|| {
                        let _ = send_post_request(&send_url, &None, args.no_validate_certs, &random_data);                        
                    })
                });

                measurements.push(Measurement {
                    name: &"Request",
                    latency: latency_result.latency.as_nanos() as u64,
                    payload_size : payload_size as u64,
                });

                println!("Average latency: {:?} : size {}", latency_result.latency, format_size(payload_size as u64));               

                payload_size += payload_size / 4; // Double the payload size for the next iteration
            }

            write_plot(
                &measurements,
                "Same Machine HTTP requests to HTTP-SYS",
                "Average MS",
                "request-latency.svg",
            )
            .expect("failed to plot");
        }
    }
}

fn send_get_request(
    url: &Url,
    proxy_url: &Option<Url>,
    validate_certs: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    let client = match proxy_url {
        Some(proxy_url) => {
            let proxy = Proxy::http(proxy_url.as_str())?;
            Client::builder()
                .proxy(proxy)
                .danger_accept_invalid_certs(validate_certs)
                .build()?
        }
        None => Client::builder()
            .danger_accept_invalid_certs(validate_certs)            
            .build()?,
    };

    let res = client.get(url.as_str()).header("Cache-Control", "no-cache").send()?;
    let body = res.text()?;
    Ok(body)
}

fn send_post_request(
    url: &Url,
    proxy_url: &Option<Url>,
    validate_certs: bool,
    random_data: &String,
) -> Result<String, Box<dyn std::error::Error>> {
    let client = match proxy_url {
        Some(proxy_url) => {
            let proxy = Proxy::http(proxy_url.as_str())?;
            Client::builder()
                .proxy(proxy)
                .danger_accept_invalid_certs(validate_certs)
                .build()?
        }
        None => Client::builder()
            .danger_accept_invalid_certs(validate_certs)
            .build()?,
    };

    let res = client
        .post(url.as_str())
        .header("Cache-Control", "no-cache")
        .body(random_data.clone())
        .send()?;

    let body = res.text()?;
    Ok(body)
}

fn generate_random_payload(data_size: usize) -> String {
    // Generate random text data
    let mut rng = thread_rng();
    let random_data: String = (0..data_size)
        .map(|_| Alphanumeric.sample(&mut rng))
        .map(char::from)
        .collect();
    random_data
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::server::Server;
    use std::{thread, time::Duration};

    #[test]
    fn test_basic_request() {
        let port_num = 1919;
        let server_url = Url::parse(&format!("http://localhost:{}/nop/", port_num)).unwrap();

        let mut server = Server::new();
        let handlers: Vec<(&Url, fn(&str) -> (String, bool))> =
            vec![(&server_url, |_| ("OK".to_string(), false))];

        server.define_handlers(handlers);

        thread::sleep(Duration::from_millis(100));

        let result = send_post_request(&server_url, &None, false, &"xxx".to_string()).unwrap();
        assert_eq!(result, "OK");

        server.kill();
        server.wait();
    }
}

fn format_size(size_in_bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;

    if size_in_bytes >= MB {
        format!("{:.1}mb", size_in_bytes as f64 / MB as f64)
    } else if size_in_bytes >= KB {
        format!("{:.1}kb", size_in_bytes as f64 / KB as f64)
    } else {
        format!("{}b", size_in_bytes)
    }
}

const FONT: &str = "Fira Code";
const PLOT_WIDTH: u32 = 800;
const PLOT_HEIGHT: u32 = 400;


pub struct Measurement<'a> {
    pub name : &'a str,
    pub latency: u64,
    pub payload_size: u64, 
}

pub fn write_plot(
    records: &Vec<Measurement>,
    caption: &str,
    y_label: &str,
    path: &str,
) -> Result<(), Box<dyn Error>> {
    let mut groups: BTreeMap<&str, Vec<&Measurement>> = BTreeMap::new();


    for record in records.iter() {
        let group = groups.entry(record.name).or_insert_with(Vec::new);
        group.push(&record);
    }

    let resolution = (PLOT_WIDTH, PLOT_HEIGHT);
    let root = SVGBackend::new(&path, resolution).into_drawing_area();

    root.fill(&WHITE)?;

    
    let y_min = records.iter().map(|m| m.latency).min().unwrap();
    let y_max = records.iter().map(|m| m.latency).max().unwrap();
    let y_diff = y_max - y_min;
    let y_padding = (y_diff / 10).min(y_min);

    let x_min = records.iter().map(|m| m.payload_size).min().unwrap();
    let x_max = records.iter().map(|m| m.payload_size).max().unwrap();

    
    let mut chart = ChartBuilder::on(&root)
        .margin(10)
        .caption(caption, (FONT, 20))
        .set_label_area_size(LabelAreaPosition::Left, 70)
        .set_label_area_size(LabelAreaPosition::Right, 70)
        .set_label_area_size(LabelAreaPosition::Bottom, 40)
        .build_cartesian_2d(1..x_max, y_min - y_padding..y_max + y_padding)?;

    chart
        .configure_mesh()
        .disable_y_mesh()
        .x_label_formatter(&|v| format_size(*v ))
        .y_label_formatter(&|v| format!("{:.1} ms", *v as f64 / 1_000_000.0))
        .x_labels(20)
        .y_labels(20)
        .y_desc(y_label)
        .x_desc("Size")
        .draw()?;

    for records in groups.values() {
        let color = BLUE;
        chart
            .draw_series(LineSeries::new(
                records
                    .iter()
                    .map(|record| (record.payload_size, record.latency)),
                color,
            ))?
            .label(records[0].name)
            .legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], color));
    }

    chart
        .configure_series_labels()
        .position(SeriesLabelPosition::UpperLeft)
        .label_font((FONT, 13))
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK)
        .draw()?;

    Ok(())
}
