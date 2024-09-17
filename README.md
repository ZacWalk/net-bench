# Network latency tester

## Usage: net-bench.exe <MODE> [OPTIONS]

## Modes

*   **server:** Starts the HTTP server.
*   **client:** Sends requests to the server and measures latency.
*   **proxy:** Acts as a proxy server.
*   **test:** Starts this app as a server and measures latency.
*   **help:** Displays this help message.

## Options

*   `-p, --port <PORT>`: Sets the port number for the server (default: 8080)
*   `-i, --ip <IP>`: Sets the IP address of the server (default: localhost)
