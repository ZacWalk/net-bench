// Copyright (c) Microsoft Corporation. All Rights Reserved.

use httpsys::{HttpInitializer, Request, RequestQueue, Response, ServerSession, UrlGroup};
use reqwest::Url;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::broadcast;
use windows::{
    core::HSTRING,
    Win32::Networking::HttpServer::{
        HttpHeaderContentType, HTTP_RECEIVE_HTTP_REQUEST_FLAGS, HTTP_REQUEST_V2,
    },
};

use crate::httpsys;

async fn return_response(queue: &RequestQueue, req: &HTTP_REQUEST_V2, result_text: &str) {
    let id = req.Base.RequestId;

    let mut resp = Response::default();
    resp.raw.Base.StatusCode = 200;
    let reason = "OK";
    resp.raw.Base.pReason = windows::core::PCSTR(reason.as_ptr());
    resp.raw.Base.ReasonLength = reason.len() as u16;

    let content_type = "application/json";
    resp.raw.Base.Headers.KnownHeaders[HttpHeaderContentType.0 as usize].RawValueLength =
        content_type.len() as u16;
    resp.raw.Base.Headers.KnownHeaders[HttpHeaderContentType.0 as usize].pRawValue =
        ::windows::core::PCSTR(content_type.as_ptr());

    resp.add_body_chunk(result_text);

    let flags = 0u32; // HTTP_SEND_RESPONSE_FLAG_DISCONNECT;

    let err = queue.async_send_response(id, flags, &resp).await;
    if err.is_err() {
        println!("handle_request failed: {:?}", err.err());
    }
}

pub(crate) struct Server {
    worker: Option<std::thread::JoinHandle<()>>,
    request_queue: Option<Arc<RequestQueue>>,
    kill_tx: Option<broadcast::Sender<String>>,
    init: Option<HttpInitializer>,
    session: Option<Arc<ServerSession>>,
    group: Option<Arc<UrlGroup>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(tx) = &self.kill_tx {
            tx.send("kill".to_string());
        }

        if let Some(handle) = self.worker.take() {
            handle.join().unwrap();
        }

        drop(self.request_queue.take());
        drop(self.group.take());
        drop(self.session.take());
        drop(self.init.take());
        drop(self.kill_tx.take());
    }
}

impl Server {
    pub fn new() -> Self {
        let init = HttpInitializer::default();
        let session = Arc::<ServerSession>::default();
        let url_group = Arc::new(UrlGroup::new(&session));
        let request_queue = Arc::new(RequestQueue::new().unwrap());
        request_queue.bind_url_group(&url_group).unwrap();
        let (kill_tx, _) = broadcast::channel::<String>(1);

        Server {
            worker: None, // Will be populated later
            request_queue: Some(request_queue),
            kill_tx: Some(kill_tx),
            init: Some(init),
            session: Some(session),
            group: Some(url_group),
        }
    }

    pub fn wait(&mut self) {
        if let Some(w) = self.worker.take() {
            w.join().unwrap();
        }
    }

    pub fn kill(&self) {
        if let Some(tx) = &self.kill_tx {
            tx.send("kill".to_string());
        }
    }

    pub fn define_handlers(&mut self, url_handlers: Vec<(&Url, fn(&str) -> (String, bool))>) {
        let mut next_url_id = 1000;
        let mut handlers: HashMap<u64, fn(&str) -> (String, bool)> = HashMap::new();

        for (url, handler_fn) in url_handlers {
            if let Some(group) = &self.group {
                group
                    .add_url(HSTRING::from(url.as_str()), next_url_id)
                    .unwrap();

                handlers.insert(next_url_id, handler_fn);
                next_url_id += 1;
            }
        }

        let rq = self.request_queue.clone(); // Clone the Option<Arc>
        let term_tx = self.kill_tx.clone(); // Clone the Option<broadcast::Sender>

        // Single background thread
        let handle = std::thread::spawn(move || {
            // Check if term_tx and rq are Some before using them
            let mut kill_channel = term_tx
                .as_ref()
                .map(|tx| tx.subscribe())
                .expect("Could not subscribe to kill channel");
            let rq = rq.as_ref();

            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                loop {
                    tokio::select! {
                        _ = async {
                            kill_channel.recv().await
                        } => {
                            println!("Shutting down server.");
                            break;
                        },
                        _ = async {
                            // Only try to receive a request if rq is Some
                            if let Some(rq) = rq {
                                let mut req = Request::default();
                                let err = rq
                                    .async_receive_request(
                                        0,
                                        HTTP_RECEIVE_HTTP_REQUEST_FLAGS::default(),
                                        &mut req,
                                    )
                                    .await;

                                if err.is_err() {
                                    println!("request fail: {:?}", err.err());
                                } else {
                                    let url = req.url();
                                    let url_context = req.raw().Base.UrlContext;

                                    if let Some(handler) = handlers.get(&url_context) {
                                        let (result, is_kill) = handler(&url);

                                        if is_kill {
                                            // Check if term_tx is Some before sending
                                            if let Some(term_tx) = &term_tx {
                                                term_tx.send("kill".to_string()).unwrap();
                                            } else {
                                                // Handle the case where term_tx is None (optional)
                                                eprintln!("Error: term_tx is None, cannot send kill signal");
                                            }
                                        }

                                        return_response(rq, &req.raw(), &result).await;
                                    } else {
                                        println!("Unknown URL context: {}", url_context);
                                    }
                                }
                            }
                        } => {}
                    }
                }
            });
        });

        self.worker = Some(handle);
    }
}
