use rand::{thread_rng, Rng};
use reqwest::blocking::get;
use std::env;
use std::fmt::Display;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct ServerExe {
    pub(crate) proc: Option<Child>,
    pub(crate) port: i32,
}

impl ServerExe {
    pub fn format_req_url<T: Display>(self: &ServerExe, path: T) -> String {
        format!("http://localhost:{}{}", self.port, path)
    }
}

impl Drop for ServerExe {
    fn drop(&mut self) {
        if let Some(mut proc) = self.proc.take() {
            proc.kill().expect("Failed to kill server");
            // Optionally wait for the process to finish
            let _ = proc.wait(); 
        }
    }
}

pub fn run_this_exe_as_server() -> ServerExe {
    let exe_path = env::current_exe().expect("Failed to get executable path");
    let mut rng = thread_rng();
    let port = rng.gen_range(3333..9999);

    println!("Current exe {:?}", exe_path);

    // Spawn the server external process
    let mut c = Command::new(exe_path);

    c.arg("server").arg("-p").arg(port.to_string());

    let proc = c
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start server. Try running 'cargo build' to make sure it is built.");

    ServerExe {
        proc: Some(proc),
        port,
    }
}

pub fn measure_latency<F, T>(f: F) -> Duration
where
    F: Fn() -> T,
{
    const MIN_ITERATIONS: usize = 10;
    const MAX_ITERATIONS: usize = 1000; // Maximum number of iterations to prevent infinite loops
    const STABLE_THRESHOLD: f64 = 0.20; // 20% change considered stable
    const OUTLIER_THRESHOLD: f64 = 3.0; // 3 standard deviations away considered an outlier

    let mut durations = Vec::new();

    for i in 0..MAX_ITERATIONS {
        let start = Instant::now();
        f();
        let duration = start.elapsed();
        durations.push(duration.as_secs_f64());

        if i >= MIN_ITERATIONS {
            // Need at least 3 measurements to calculate mean and std dev
            let mean = durations.iter().sum::<f64>() / durations.len() as f64;
            let variance = durations
                .iter()
                .map(|d| {
                    let diff = d - mean;
                    diff * diff
                })
                .sum::<f64>()
                / durations.len() as f64;
            let std_dev = variance.sqrt();

            // Remove outliers
            durations.retain(|d| {
                let diff = (*d - mean).abs();
                diff / std_dev <= OUTLIER_THRESHOLD
            });

            if durations.len() > MIN_ITERATIONS {
                // Check for stability
                let is_stable = durations.iter().all(|d| {
                    let diff = (d - mean).abs();
                    diff / mean <= STABLE_THRESHOLD
                });

                if is_stable {
                    return Duration::from_secs_f64(mean);
                }
            }
        }
    }

    panic!("Failed to converge after {} iterations", MAX_ITERATIONS);
}
