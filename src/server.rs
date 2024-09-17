// Copyright (c) Microsoft Corporation. All Rights Reserved.

use httpsys::{HttpInitializer, Request, RequestQueue, Response, ServerSession, UrlGroup};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::broadcast;
use windows::{
    core::HSTRING,
    Win32::Networking::HttpServer::{
        HttpHeaderContentType, HTTP_RECEIVE_HTTP_REQUEST_FLAGS, HTTP_REQUEST_V2,
    },
};

use crate::httpsys;

async fn handle_request<F>(
    queue: &RequestQueue,
    req: &HTTP_REQUEST_V2,
    url: &str,
    process_request: F,
) where
    F: Fn(&str) -> String,
{
    let id = req.Base.RequestId;
    let result = process_request(url);

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

    resp.add_body_chunk(result);

    let flags = 0u32; // HTTP_SEND_RESPONSE_FLAG_DISCONNECT;

    let err = queue.async_send_response(id, flags, &resp).await;
    if err.is_err() {
        println!("handle_request failed: {:?}", err.err());
    }
}

pub(crate) struct Server {
    handles: Vec<std::thread::JoinHandle<()>>,
    request_queue: Arc<RequestQueue>,
    term_tx: broadcast::Sender<String>,
    init: HttpInitializer,
    session: Arc<ServerSession>,
    group: Arc<UrlGroup>,
}

impl Server {
    pub fn new() -> std::io::Result<Self> {
        let num_cores = num_cpus::get();
        let init = HttpInitializer::default();
        let session = Arc::<ServerSession>::default();
        let url_group = Arc::new(UrlGroup::new(&session));
        let request_queue = Arc::new(RequestQueue::new().unwrap());
        request_queue.bind_url_group(&url_group).unwrap();
        let (term_tx, _) = broadcast::channel::<String>(1);

        Ok(Server {
            handles: vec![], // Will be populated later
            request_queue,
            term_tx,
            init,
            session,
            group: url_group,
        })
    }

    pub fn wait(self) {
        for handle in self.handles {
            handle.join().unwrap();
        }
    }

    pub fn kill(self) {
        self.term_tx.send("kill".to_string());
    }

    pub fn define_handlers(&mut self, url_handlers: Vec<(&str, fn(&str) -> (String, bool))>) {
        let mut next_url_id = 1000;
        let mut handlers: HashMap<u64, fn(&str) -> (String, bool)> = HashMap::new();

        for (url, handler_fn) in url_handlers {
            self.group.add_url(HSTRING::from(url), next_url_id).unwrap();

            handlers.insert(next_url_id, handler_fn);
            next_url_id += 1;
        }

        let num_cores = num_cpus::get();
        let rq = self.request_queue.clone();        

        for core_id in 0..num_cores {
            let rq = rq.clone();            
            let handlers = handlers.clone(); // Clone the handlers for each thread
            let term_tx = self.term_tx.clone();

            let handle = std::thread::spawn(move || {
                let mut kill_channel = term_tx.subscribe();
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async move {
                    loop {
                        tokio::select! {
                            _ = kill_channel.recv() => {
                                println!("Shutdown core {}.", core_id);
                                break;
                            },
                            _ = async {
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

                                        let kill_sender = term_tx.clone();

                                        let h : Box<dyn Fn(&str) -> String> = Box::new(move |url| { 
                                            let (result, is_kill) = handler(url);
                                            if is_kill {
                                                kill_sender.send("kill".to_string()).unwrap(); // Handle potential send error
                                            }
                                            result // Assuming this is the intended return value
                                        });

                                        handle_request(&rq, &req.raw(), &url, h).await;
                                    } else {
                                        println!("Unknown URL context: {}", url_context);
                                    }
                                }
                            } => {}
                        }
                    }
                });
            });

            self.handles.push(handle); // Add the handle to the server's handles
        }
    }
}

// pub(crate) fn start_handling_requests(url_handlers: Vec<(&str, fn(&str) -> String)>) -> std::io::Result<Server> {
//     let num_cores = num_cpus::get();
//     let init = HttpInitializer::default();
//     let session = Arc::<ServerSession>::default();
//     let url_group = Arc::new(UrlGroup::new(&session));
//     let mut handlers : HashMap<u64, fn(&str) -> String> = HashMap::new();
//     let mut next_url_id = 1000;

//     // Add URLs and their handlers dynamically
//     for (url, handler_fn) in url_handlers {
//         url_group
//             .add_url(
//                 HSTRING::from(url),
//                 next_url_id,
//             )
//             .unwrap();

//             handlers.insert(next_url_id, handler_fn);
//             next_url_id += 1;
//     }

//     let request_queue = Arc::new(RequestQueue::new().unwrap());
//     request_queue.bind_url_group(&url_group).unwrap();

//     let (term_tx, _) = broadcast::channel::<String>(1);

//     let mut handles = vec![];
//     for core_id in 0..num_cores {
//         let rq = request_queue.clone();
//         let mut term_rx = term_tx.subscribe();
//         let handlers : HashMap<u64, fn(&str) -> String> = HashMap::new();

//         let handle = std::thread::spawn(move || {
//             let rt = tokio::runtime::Runtime::new().unwrap();
//             rt.block_on(async move {
//                 loop {
//                     tokio::select! {
//                       _ = term_rx.recv() =>{
//                         println!("Shutdown core {}.", core_id);
//                         break;
//                       }
//                       _ = async{
//                         let mut req = Request::default();
//                         let err = rq
//                             .async_receive_request(
//                                 0,
//                                 HTTP_RECEIVE_HTTP_REQUEST_FLAGS::default(),
//                                 &mut req,
//                             )
//                             .await;

//                             if err.is_err() {
//                                 println!("request fail: {:?}", err.err());
//                             }
//                             else
//                             {
//                                 let url = req.url();
//                                 let url_context = req.raw().Base.UrlContext;

//                                 // Find the matching handler function
//                                 if let Some(handler) = handlers.get(&url_context) {
//                                     handle_request(&rq, &req.raw(), &url, handler).await;
//                                 } else {
//                                     // Handle unknown URL context (optional)
//                                     println!("Unknown URL context: {}", url_context);
//                                 }
//                             }
//                       } => {}
//                     }
//                 }
//             });
//         });
//         handles.push(handle);
//     }

//     Ok(Server {
//         handles,
//         request_queue,
//         term_tx,
//         init,
//         session,
//         group: url_group,
//     })
// }
