use std::{
    ffi::CStr,
    future::Future,
    os::raw::c_char,
    pin::Pin,
    sync::Arc,
    sync::Mutex,
    task::{Context, Poll, Waker},
};
use windows::{
    core::{Error, HRESULT, HSTRING, PCSTR},
    Win32::{
        Foundation::{
            GetLastError, ERROR_INSUFFICIENT_BUFFER, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, HANDLE,
            NO_ERROR, WIN32_ERROR,
        },
        Networking::HttpServer::{
            HttpAddUrlToUrlGroup, HttpCloseRequestQueue, HttpCloseServerSession, HttpCloseUrlGroup,
            HttpCreateRequestQueue, HttpCreateServerSession, HttpCreateUrlGroup,
            HttpDataChunkFromMemory, HttpInitialize, HttpReceiveHttpRequest, HttpSendHttpResponse,
            HttpServerBindingProperty, HttpSetUrlGroupProperty, HttpTerminate, HTTPAPI_VERSION,
            HTTP_BINDING_INFO, HTTP_DATA_CHUNK, HTTP_INITIALIZE_CONFIG, HTTP_INITIALIZE_SERVER,
            HTTP_RECEIVE_HTTP_REQUEST_FLAGS, HTTP_REQUEST_V2, HTTP_RESPONSE_V2,
            HTTP_SERVER_PROPERTY,
        },
        System::IO::{BindIoCompletionCallback, GetOverlappedResult, OVERLAPPED},
    },
};

struct OverlappedFuture {
    handle: HANDLE,
    overlapped: OVERLAPPED,
}

impl OverlappedFuture {
    pub fn new(handle: HANDLE, overlapped: OVERLAPPED) -> Self {
        OverlappedFuture { handle, overlapped }
    }
}

impl std::future::Future for OverlappedFuture {
    type Output = std::io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut bytes_transferred = 0;
        let overlapped_ptr = &self.overlapped as *const _ as *mut OVERLAPPED;

        let result = unsafe {
            GetOverlappedResult(self.handle, overlapped_ptr, &mut bytes_transferred, false)
        };

        if result != false {
            Poll::Ready(Ok(()))
        } else {
            let error = unsafe { GetLastError() };
            if error == ERROR_IO_INCOMPLETE {
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(Err(std::io::Error::from_raw_os_error(error.0 as i32)))
            }
        }
    }
}

pub fn register_iocp_handle(h: HANDLE) -> Result<(), Error> {
    let ok = unsafe { BindIoCompletionCallback(h, Some(private_callback), 0) };
    ok.ok()
}

unsafe extern "system" fn private_callback(
    dwerrorcode: u32,
    dwnumberofbytestransfered: u32,
    lpoverlapped: *mut OVERLAPPED,
) {
    let e = Error::from(WIN32_ERROR(dwerrorcode));
    if e.code().is_err() {}

    let wrap_ptr: *mut OverlappedWrap = lpoverlapped as *mut OverlappedWrap;
    let _wrap = Arc::from_raw(wrap_ptr);
    let wrap: &mut OverlappedWrap = &mut *wrap_ptr;

    if dwerrorcode != 0x80000005 && e.code().is_err() {
        wrap.err = e;
    } else {
        wrap.len = dwnumberofbytestransfered;
    }
    wrap.as_obj.wake();
}

#[derive(Debug)]
struct SharedState {
    completed: bool,
    waker: Option<Waker>,
}

pub struct AsyncWaitObject {
    shared_state: Arc<Mutex<SharedState>>,
}

pub struct AwaitableToken {
    shared_state: Arc<Mutex<SharedState>>,
}

impl Default for AsyncWaitObject {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncWaitObject {
    pub fn new() -> AsyncWaitObject {
        AsyncWaitObject {
            shared_state: Arc::new(Mutex::new(SharedState {
                completed: false,
                waker: None,
            })),
        }
    }

    pub fn wake(&self) {
        let mut shared_state = self.shared_state.lock().unwrap();
        shared_state.completed = true;
        if let Some(waker) = shared_state.waker.take() {
            waker.wake()
        }
    }

    pub fn reset(&mut self) {
        self.shared_state = Arc::new(Mutex::new(SharedState {
            completed: false,
            waker: None,
        }));
    }

    pub fn get_await_token(&self) -> AwaitableToken {
        AwaitableToken {
            shared_state: self.shared_state.clone(),
        }
    }
}

impl Future for AwaitableToken {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut shared_state = self.shared_state.lock().unwrap();
        if shared_state.completed {
            Poll::Ready(())
        } else {
            shared_state.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

unsafe impl Send for OverlappedWrap {}
unsafe impl Sync for OverlappedWrap {}

#[repr(C)]
pub struct OverlappedWrap {
    o: OVERLAPPED,
    as_obj: AsyncWaitObject,
    err: Error,
    len: u32,
}

impl Default for OverlappedWrap {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlappedWrap {
    pub fn new() -> Self {
        OverlappedWrap {
            o: OVERLAPPED::default(),
            as_obj: AsyncWaitObject::new(),
            err: Error::OK,
            len: 0,
        }
    }
}

pub struct OverlappedObject {
    o: OverlappedWrap,
}

impl Default for OverlappedObject {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlappedObject {
    pub fn new() -> Self {
        OverlappedObject {
            o: OverlappedWrap::new(),
        }
    }

    pub fn get(&self) -> *const OVERLAPPED {
        let ow_ptr: *const OverlappedWrap = std::ptr::addr_of!(self.o);
        let ow_cast_ptr: *const OVERLAPPED = ow_ptr as *const OVERLAPPED;
        ow_cast_ptr
    }

    pub fn get_mut(&self) -> *mut OVERLAPPED {
        let ow_ptr: *const OverlappedWrap = std::ptr::addr_of!(self.o);
        let ow_cast_ptr: *mut OVERLAPPED = ow_ptr as *mut OVERLAPPED;
        ow_cast_ptr
    }

    pub async fn wait(&self) {
        self.o.as_obj.get_await_token().await;
    }

    pub fn get_ec(&self) -> Error {
        self.o.err.clone()
    }

    pub fn get_len(&self) -> u32 {
        self.o.len
    }
}

static G_HTTP_VERSION: HTTPAPI_VERSION = HTTPAPI_VERSION {
    HttpApiMajorVersion: 2,
    HttpApiMinorVersion: 0,
};

pub struct HttpInitializer { running: bool }

impl HttpInitializer {
    pub fn default() -> HttpInitializer {
        let ec = unsafe {
            HttpInitialize(
                G_HTTP_VERSION,
                HTTP_INITIALIZE_SERVER | HTTP_INITIALIZE_CONFIG,
                None,
            )
        };
        let err = Error::from(HRESULT(ec.try_into().unwrap()));
        assert_eq!(err, Error::OK);
        Self { running : true }
    }
}

impl Drop for HttpInitializer {
    fn drop(&mut self) {
        let ec = unsafe { HttpTerminate(HTTP_INITIALIZE_SERVER | HTTP_INITIALIZE_CONFIG, None) };
        let err = Error::from(HRESULT(ec.try_into().unwrap()));
        assert_eq!(err, Error::OK);
    }
}

pub struct ServerSession {
    id: u64,
}

impl ServerSession {
    pub fn new() -> ServerSession {
        let mut id: u64 = 0;
        let ec = unsafe { HttpCreateServerSession(G_HTTP_VERSION, std::ptr::addr_of_mut!(id), 0) };
        let err = Error::from(HRESULT(ec.try_into().unwrap()));
        assert_eq!(err, Error::OK);
        ServerSession { id }
    }
}
impl Default for ServerSession {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ServerSession {
    fn drop(&mut self) {
        let ec = unsafe { HttpCloseServerSession(self.id) };
        let err = Error::from(HRESULT(ec.try_into().unwrap()));
        assert_eq!(err, Error::OK);
    }
}

pub struct UrlGroup {
    _session: Arc<ServerSession>,
    id: u64,
}

impl UrlGroup {
    pub fn new(session: &Arc<ServerSession>) -> UrlGroup {
        let mut id: u64 = 0;
        let ec = unsafe { HttpCreateUrlGroup(session.id, std::ptr::addr_of_mut!(id), 0) };
        let err = Error::from(HRESULT(ec.try_into().unwrap()));
        assert_eq!(err, Error::OK);
        UrlGroup {
            _session: Arc::clone(session),
            id,
        }
    }

    unsafe fn set_property(
        &self,
        property: HTTP_SERVER_PROPERTY,
        propertyinformation: *const ::core::ffi::c_void,
        propertyinformationlength: u32,
    ) -> Result<(), Error> {
        let ec = unsafe {
            HttpSetUrlGroupProperty(
                self.id,
                property,
                propertyinformation,
                propertyinformationlength,
            )
        };
        let err = Error::from(HRESULT(ec.try_into().unwrap()));
        err.code().ok()
    }

    pub fn set_binding_info(&self, info: &HTTP_BINDING_INFO) -> Result<(), Error> {
        let info_ptr: *const HTTP_BINDING_INFO = info;
        unsafe {
            self.set_property(
                HttpServerBindingProperty,
                info_ptr as *const std::ffi::c_void,
                std::mem::size_of::<HTTP_BINDING_INFO>() as u32,
            )
        }
    }

    pub fn add_url(&self, url: HSTRING, context: u64) -> Result<(), Error> {
        let ec = unsafe { HttpAddUrlToUrlGroup(self.id, &url, context, 0) };
        let err = Error::from(HRESULT(ec.try_into().unwrap()));
        err.code().ok()
    }
}

impl Drop for UrlGroup {
    fn drop(&mut self) {
        let ec = unsafe { HttpCloseUrlGroup(self.id) };
        let err = Error::from(HRESULT(ec.try_into().unwrap()));
        assert_eq!(err, Error::OK);
    }
}

#[repr(C)]
pub struct Request {
    raw: HTTP_REQUEST_V2,
    buff: [u8; 1024 * 4],
}

impl Default for Request {
    fn default() -> Request {
        Request {
            raw: HTTP_REQUEST_V2::default(),
            buff: [0; 1024 * 4],
        }
    }
}

impl Request {
    pub fn raw(&mut self) -> &mut HTTP_REQUEST_V2 {
        &mut self.raw
    }

    pub fn size() -> u32 {
        std::mem::size_of::<Request>() as u32
    }

    pub fn url(&self) -> String {
        if self.raw.Base.pRawUrl != PCSTR::null() {
            let c_ptr = self.raw.Base.pRawUrl.0 as *const c_char;
            unsafe {
                let cs = CStr::from_ptr(c_ptr);
                cs.to_string_lossy().into_owned()
            }
        } else {
            String::default()
        }
    }
}
unsafe impl Send for Request {}
unsafe impl Sync for Request {}

#[derive(Default)]
#[repr(C)]
pub struct Response {
    pub(crate) raw: HTTP_RESPONSE_V2,
    data_chunks: Box<HTTP_DATA_CHUNK>,
    strings: String,
}
unsafe impl Send for Response {}
unsafe impl Sync for Response {}

impl Response {
    pub fn raw(&self) -> *const HTTP_RESPONSE_V2 {
        &self.raw
    }

    pub fn add_body_chunk(&mut self, data: String) {
        self.strings = data;

        let mut chunk = Box::<HTTP_DATA_CHUNK>::default();
        chunk.DataChunkType = HttpDataChunkFromMemory;
        chunk.Anonymous.FromMemory.BufferLength = self.strings.len() as u32;
        chunk.Anonymous.FromMemory.pBuffer = self.strings.as_mut_ptr() as *mut std::ffi::c_void;

        self.raw.Base.EntityChunkCount = 1;
        self.raw.Base.pEntityChunks = &mut *chunk;

        self.data_chunks = chunk;
    }
}

pub struct RequestQueue {
    h: HANDLE,
}

unsafe impl Send for RequestQueue {}
unsafe impl Sync for RequestQueue {}

impl RequestQueue {
    pub fn new() -> Result<RequestQueue, Error> {
        let mut h: HANDLE = HANDLE::default();
        let ec = unsafe {
            HttpCreateRequestQueue(G_HTTP_VERSION, None, None, 0, std::ptr::addr_of_mut!(h))
        };
        let err = Error::from(HRESULT(ec.try_into().unwrap()));
        if err.code().is_err() {
            Err(err)
        } else {
            assert!(!h.is_invalid());
            register_iocp_handle(h).unwrap();
            Ok(RequestQueue { h })
        }
    }

    pub fn bind_url_group(&self, url_group: &UrlGroup) -> Result<(), Error> {
        let info = HTTP_BINDING_INFO {
            Flags: windows::Win32::Networking::HttpServer::HTTP_PROPERTY_FLAGS { _bitfield: 1 },
            RequestQueueHandle: self.h,
        };
        url_group.set_binding_info(&info)
    }

    pub async fn async_receive_request(
        &self,
        requestid: u64,
        flags: HTTP_RECEIVE_HTTP_REQUEST_FLAGS,
        requestbuffer: &mut Request,
    ) -> Result<u32, Error> {
        let optr = Arc::new(OverlappedObject::new());
        let ec = unsafe {
            HttpReceiveHttpRequest(
                self.h,
                requestid,
                flags,
                requestbuffer.raw(),
                Request::size(),
                None,
                Some(optr.get()),
            )
        };
        let err = WIN32_ERROR(ec);
        if err == ERROR_IO_PENDING || err == NO_ERROR {
            std::mem::forget(optr.clone());
            optr.wait().await;
            let async_err = optr.get_ec();
            if async_err == Error::OK {
                Ok(optr.get_len())
            } else {
                Err(async_err)
            }
        } else {
            assert_ne!(err, ERROR_INSUFFICIENT_BUFFER);
            Err(Error::from(err))
        }
    }

    pub async fn async_send_response(
        &self,
        requestid: u64,
        flags: u32,
        httpresponse: &Response,
    ) -> Result<u32, Error> {
        let optr = Arc::new(OverlappedObject::new());
        let ec = unsafe {
            HttpSendHttpResponse(
                self.h,
                requestid,
                flags,
                httpresponse.raw(),
                None,
                None,
                None,
                0,
                Some(optr.get()),
                None,
            )
        };
        let err = WIN32_ERROR(ec);

        if err == ERROR_IO_PENDING || err == NO_ERROR {
            std::mem::forget(optr.clone());
            let _ = OverlappedFuture::new(self.h, unsafe { *optr.get() }).await;
            optr.wait().await;
            let async_err = optr.get_ec();
            if async_err == Error::OK {
                Ok(optr.get_len())
            } else {
                Err(async_err)
            }
        } else {
            Err(Error::from(err))
        }
    }

    pub fn close(&mut self) {
        if self.h.is_invalid() {
            return;
        }
        let ec = unsafe { HttpCloseRequestQueue(self.h) };
        let err = Error::from(HRESULT(ec.try_into().unwrap()));
        assert_eq!(err, Error::OK);
        self.h = HANDLE(0);
    }
}

impl Drop for RequestQueue {
    fn drop(&mut self) {
        self.close()
    }
}
